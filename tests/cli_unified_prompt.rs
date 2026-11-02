//! Integration tests for the unified install prompt (spec/install-hooks.md
//! "Unified install prompt", HOOK-180..186) and the fixed-choice prompt rules
//! (spec/cli.md "Prompts", CLI-257..259).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo with a source install hook, a source uninstall hook, and items
//! that declare their own install and uninstall hooks. `MIND_TTY=1` (HOOK-109)
//! plus a scripted stdin reaches the interactive branches. Every hook touches
//! an absolute sentinel, so a test sees exactly which hooks ran.

use std::io::Write as _;
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
    /// A source named `name` with a source install hook (`src-install`), a
    /// source uninstall hook (`src-uninstall`), and two items: the skill
    /// `widget`, whose install and uninstall hooks touch `item-install` and
    /// `item-uninstall`, and the hookless rule `style`.
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-up-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(
            &source.join("mind.toml"),
            &format!(
                concat!(
                    "[[hooks]]\n",
                    "name = \"setup\"\n",
                    "run = \"{}\"\n",
                    "event = \"install\"\n",
                    "\n",
                    "[[hooks]]\n",
                    "name = \"teardown\"\n",
                    "run = \"{}\"\n",
                    "event = \"uninstall\"\n",
                ),
                sb.touch("src-install"),
                sb.touch("src-uninstall"),
            ),
        );
        write(
            &source.join("skills/widget/SKILL.md"),
            &format!(
                "---\ndescription: widget\ninstall: {}\nuninstall: {}\n---\n# widget\n",
                sb.touch("item-install"),
                sb.touch("item-uninstall"),
            ),
        );
        write(
            &source.join("rules/style.md"),
            "---\ndescription: style\n---\n# style\n",
        );
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    fn run(&self, args: &[&str], tty: bool, stdin: &str) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env("MIND_TTY", if tty { "1" } else { "0" })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn mind");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Run `mind` at a (simulated) terminal with `stdin` as the replies.
    fn tty(&self, args: &[&str], stdin: &str) -> Run {
        self.run(args, true, stdin)
    }

    /// Run `mind` without a terminal, with `stdin` piped in anyway.
    fn piped(&self, args: &[&str], stdin: &str) -> Run {
        self.run(args, false, stdin)
    }

    fn ran(&self, sentinel: &str) -> bool {
        self.base.join(sentinel).exists()
    }

    fn clear(&self, sentinel: &str) {
        let _ = std::fs::remove_file(self.base.join(sentinel));
    }

    fn touch(&self, sentinel: &str) -> String {
        format!("touch {}", self.base.join(sentinel).display())
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn identity(&self) -> String {
        format!(
            "local/{}/{}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
    }

    fn installed(&self, rel: &str) -> bool {
        std::fs::symlink_metadata(self.claude_home.join(rel)).is_ok()
    }

    fn registered(&self) -> bool {
        std::fs::read_to_string(self.mind_home.join("sources.json"))
            .is_ok_and(|s| s.contains(&self.identity()))
    }

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    /// Meld and install everything with every hook run unattended.
    fn meld_all(&self) {
        let r = self.piped(&["meld", &self.spec(), "--yes", "--accept-hooks"], "");
        assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
        assert!(self.ran("src-install") && self.ran("item-install"));
        self.clear("src-install");
        self.clear("item-install");
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

// ---------------------------------------------------------------------------
// meld
// ---------------------------------------------------------------------------

#[test]
fn meld_shows_one_plan_with_every_hook_before_any_runs() {
    // spec: HOOK-180 HOOK-183 HOOK-184
    let sb = Sandbox::new("plan");
    let r = sb.tty(&["meld", &sb.spec()], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    let out = &r.stdout;
    assert!(out.contains("====== hooks: 2 ======"), "{out}");
    assert!(out.contains("[1] setup"), "the source hook first: {out}");
    // A frontmatter hook has no name, so its command labels it (HOOK-51).
    assert!(out.contains("[2] touch "), "then the item's: {out}");
    assert!(out.contains("Item:    skill:widget"), "{out}");
    assert!(
        out.contains(&format!(
            "Runs in: {}",
            sb.mind_home.join("store/skill/widget").display()
        )),
        "an item hook names where it runs: {out}"
    );
    assert!(
        out.contains("skills/widget/SKILL.md (the source copy, before token expansion)")
            || out.contains("skills/widget (the source copy, before token expansion)"),
        "an item hook names its clone path to inspect: {out}"
    );
    assert!(out.contains("[Y/s/r/n]"), "{out}");
    // One prompt: no per-hook disclosure was shown.
    assert!(!out.contains("====== hook: "), "{out}");
    // Declined: no hook ran, nothing installed, but the source stays melded.
    assert!(!sb.ran("src-install") && !sb.ran("item-install"), "{out}");
    assert!(!sb.installed("skills/widget"), "{out}");
    assert!(sb.registered(), "`n` leaves the source registered: {out}");
}

#[test]
fn meld_yes_runs_every_listed_hook_without_another_prompt() {
    // spec: HOOK-181 HOOK-183
    let sb = Sandbox::new("plan-y");
    let r = sb.tty(&["meld", &sb.spec()], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.ran("src-install"), "{}", r.stdout);
    assert!(sb.ran("item-install"), "{}", r.stdout);
    assert!(sb.installed("skills/widget") && sb.installed("rules/style.md"));
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);
    // HOOK-183: the source hook ran after the plan was answered.
    let plan = r.stdout.find("[Y/s/r/n]").expect("the plan prompt");
    let ran = r.stdout.find("running").expect("a running line");
    assert!(plan < ran, "{}", r.stdout);
}

#[test]
fn meld_skip_installs_the_items_and_runs_no_hook() {
    // spec: HOOK-181
    let sb = Sandbox::new("plan-s");
    let r = sb.tty(&["meld", &sb.spec()], "s\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.installed("skills/widget"), "{}", r.stdout);
    assert!(
        !sb.ran("src-install") && !sb.ran("item-install"),
        "{}",
        r.stdout
    );
}

#[test]
fn meld_review_prompts_each_hook_on_its_own() {
    // spec: HOOK-181
    let sb = Sandbox::new("plan-r");
    let r = sb.tty(&["meld", &sb.spec()], "r\ny\nn\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert_eq!(r.stdout.matches("====== hook: ").count(), 2, "{}", r.stdout);
    assert!(
        sb.ran("src-install"),
        "the first prompt said y: {}",
        r.stdout
    );
    assert!(!sb.ran("item-install"), "the second said n: {}", r.stdout);
    assert!(sb.installed("skills/widget"), "{}", r.stdout);
}

#[test]
fn meld_plan_eof_takes_the_default_and_runs_everything() {
    // spec: HOOK-181 CLI-258
    let sb = Sandbox::new("plan-eof");
    let r = sb.tty(&["meld", &sb.spec()], "");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.installed("skills/widget"), "{}", r.stdout);
    assert!(
        sb.ran("src-install") && sb.ran("item-install"),
        "{}",
        r.stdout
    );
}

#[test]
fn meld_plan_rejects_an_unclear_reply_and_asks_again() {
    // spec: CLI-257 HOOK-181
    let sb = Sandbox::new("plan-reask");
    let r = sb.tty(&["meld", &sb.spec()], "maybe\nn\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("please answer y, s, r, or n"),
        "{}",
        r.stdout
    );
    assert_eq!(r.stdout.matches("[Y/s/r/n]").count(), 2, "{}", r.stdout);
    assert!(
        !sb.installed("skills/widget"),
        "the `n` decides: {}",
        r.stdout
    );
    assert!(!sb.ran("src-install"), "{}", r.stdout);
}

#[test]
fn meld_review_abort_unregisters_a_fresh_source() {
    // spec: HOOK-181 HOOK-183 -- `r`, then `a` at the required source hook:
    // the fresh meld is undone and no item installs.
    let sb = Sandbox::new("plan-abort");
    let r = sb.tty(&["meld", &sb.spec()], "r\na\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("aborted; nothing installed"),
        "{}",
        r.stdout
    );
    assert!(!sb.registered(), "{}", r.stdout);
    assert!(!sb.installed("skills/widget"), "{}", r.stdout);
    assert!(
        !sb.ran("src-install") && !sb.ran("item-install"),
        "{}",
        r.stdout
    );
}

#[test]
fn remeld_offers_a_skipped_source_hook_in_a_hooks_only_plan() {
    // spec: HOOK-180 HOOK-182 -- a source hook skipped at the first meld is
    // re-offered (HOOK-60) through the plan, not its own prompt.
    let sb = Sandbox::new("plan-remeld");
    let first = sb.tty(&["meld", &sb.spec()], "s\n");
    assert!(first.success, "{}\n{}", first.stdout, first.stderr);
    assert!(!sb.ran("src-install"), "{}", first.stdout);
    let r = sb.tty(&["meld", &sb.spec()], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[1] setup"), "{}", r.stdout);
    assert!(r.stdout.contains("[Y/n/r]"), "{}", r.stdout);
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);
    assert!(sb.ran("src-install"), "{}", r.stdout);
}

#[test]
fn learn_of_an_item_with_a_hook_asks_the_plan() {
    // spec: HOOK-180 HOOK-181 -- a direct `learn` lists the item and its hook
    // and asks once; a hookless `learn` asks nothing (covered elsewhere).
    let sb = Sandbox::new("plan-learn");
    let reg = sb.piped(&["meld", &sb.spec(), "--register-only"], "");
    assert!(reg.success, "{}\n{}", reg.stdout, reg.stderr);
    let r = sb.tty(&["learn", "skill:widget"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("would learn 1 item(s):"), "{}", r.stdout);
    assert!(r.stdout.contains("[Y/s/r/n]"), "{}", r.stdout);
    assert!(!sb.installed("skills/widget"), "{}", r.stdout);
    let r = sb.tty(&["learn", "skill:widget"], "s\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.installed("skills/widget"), "{}", r.stdout);
    assert!(!sb.ran("item-install"), "{}", r.stdout);
}

#[test]
fn meld_register_only_asks_a_hooks_only_plan() {
    // spec: HOOK-182
    let sb = Sandbox::new("plan-reg");
    let r = sb.tty(&["meld", &sb.spec(), "--register-only"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("====== hooks: 1 ======"), "{}", r.stdout);
    assert!(r.stdout.contains("[Y/n/r]"), "{}", r.stdout);
    assert!(!r.stdout.contains("[Y/s/r/n]"), "{}", r.stdout);
    assert!(!sb.ran("src-install"), "{}", r.stdout);
    assert!(sb.registered(), "{}", r.stdout);
}

#[test]
fn meld_with_yes_asks_only_about_the_hooks() {
    // spec: HOOK-182
    let sb = Sandbox::new("plan-yes");
    let r = sb.tty(&["meld", &sb.spec(), "--yes"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[Y/n/r]"), "{}", r.stdout);
    assert!(
        sb.installed("skills/widget"),
        "--yes answered the items: {}",
        r.stdout
    );
    assert!(
        !sb.ran("src-install") && !sb.ran("item-install"),
        "{}",
        r.stdout
    );
}

#[test]
fn meld_with_every_hook_accepted_asks_the_item_question_alone() {
    // spec: HOOK-182
    let sb = Sandbox::new("plan-acc");
    let r = sb.tty(&["meld", &sb.spec(), "--accept-hooks"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[Y/n]"), "{}", r.stdout);
    assert!(!r.stdout.contains("[Y/s/r/n]"), "{}", r.stdout);
    assert!(!sb.installed("skills/widget"), "{}", r.stdout);
    // spec: HOOK-183 -- an accepted source hook is not held back for the plan.
    assert!(sb.ran("src-install"), "{}", r.stdout);
    assert!(!sb.ran("item-install"), "{}", r.stdout);
}

#[test]
fn meld_without_a_tty_is_unchanged() {
    // spec: HOOK-182 CLI-259 -- a non-TTY meld keeps CLI-23's register-only
    // fallback and HOOK-22's skip: no prompt, no hook, no refusal.
    let sb = Sandbox::new("plan-pipe");
    let r = sb.piped(&["meld", &sb.spec()], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("[Y/s/r/n]"), "{}", r.stdout);
    assert!(!sb.ran("src-install"), "{}", r.stdout);
    assert!(!sb.installed("skills/widget"), "{}", r.stdout);
    assert!(sb.registered(), "{}", r.stdout);
}

// ---------------------------------------------------------------------------
// upgrade
// ---------------------------------------------------------------------------

#[test]
fn upgrade_runs_no_source_hook_before_its_plan_is_answered() {
    // spec: HOOK-180 HOOK-183
    let sb = Sandbox::new("plan-up");
    sb.meld_all();
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        &format!(
            "---\ndescription: widget v2\ninstall: {}\nuninstall: {}\n---\n# widget\n",
            sb.touch("item-install"),
            sb.touch("item-uninstall"),
        ),
    );

    let r = sb.tty(&["upgrade"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("apply these upgrades?"), "{}", r.stdout);
    assert!(r.stdout.contains("[1] setup"), "{}", r.stdout);
    assert!(
        r.stdout.contains("aborted; nothing changed"),
        "{}",
        r.stdout
    );
    assert!(
        !sb.ran("src-install"),
        "declining must run no source hook: {}",
        r.stdout
    );

    let r = sb.tty(&["upgrade"], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.ran("src-install"), "{}", r.stdout);
    assert!(r.stdout.contains("upgraded skill:widget"), "{}", r.stdout);
}

#[test]
fn upgrade_without_a_tty_or_yes_refuses_before_any_hook() {
    // spec: CLI-259 HOOK-183
    let sb = Sandbox::new("plan-up-pipe");
    sb.meld_all();
    sb.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# v2\n");
    let r = sb.piped(&["upgrade"], "y\n");
    assert!(!r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
    assert!(!sb.ran("src-install"), "{}", r.stdout);
}

// ---------------------------------------------------------------------------
// unmeld
// ---------------------------------------------------------------------------

#[test]
fn unmeld_lists_its_uninstall_hooks_with_the_removal_confirmation() {
    // spec: HOOK-186
    let sb = Sandbox::new("plan-un");
    sb.meld_all();

    let r = sb.tty(&["unmeld", &sb.identity()], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("====== hooks: 2 ======"), "{}", r.stdout);
    assert!(
        r.stdout.contains("[1] touch ") && r.stdout.contains("Item:    skill:widget"),
        "item hook first: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("[2] teardown"),
        "then the source's: {}",
        r.stdout
    );
    // A removal defaults to no.
    assert!(r.stdout.contains("[y/s/r/N]"), "{}", r.stdout);
    assert!(
        r.stdout.contains("cancelled; nothing removed"),
        "{}",
        r.stdout
    );
    assert!(sb.registered() && sb.installed("skills/widget"));
    assert!(!sb.ran("item-uninstall") && !sb.ran("src-uninstall"));

    let r = sb.tty(&["unmeld", &sb.identity()], "s\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(
        !sb.registered() && !sb.installed("skills/widget"),
        "{}",
        r.stdout
    );
    assert!(
        !sb.ran("item-uninstall") && !sb.ran("src-uninstall"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);
}

#[test]
fn unmeld_yes_runs_the_listed_uninstall_hooks() {
    // spec: HOOK-186
    let sb = Sandbox::new("plan-un-y");
    sb.meld_all();
    let r = sb.tty(&["unmeld", &sb.identity()], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(
        sb.ran("item-uninstall") && sb.ran("src-uninstall"),
        "{}",
        r.stdout
    );
    assert!(!sb.registered(), "{}", r.stdout);
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);
}

#[test]
fn unmeld_plan_eof_and_enter_remove_nothing() {
    // spec: HOOK-186 CLI-258 CLI-260 -- a removal's default is no, so Enter and EOF
    // cancel.
    let sb = Sandbox::new("plan-un-eof");
    sb.meld_all();
    for input in ["", "\n"] {
        let r = sb.tty(&["unmeld", &sb.identity()], input);
        assert!(r.success, "{}\n{}", r.stdout, r.stderr);
        assert!(
            r.stdout.contains("cancelled; nothing removed"),
            "{input:?}: {}",
            r.stdout
        );
        assert!(sb.registered() && sb.installed("skills/widget"));
        assert!(!sb.ran("item-uninstall") && !sb.ran("src-uninstall"));
    }
}

#[test]
fn unmeld_keep_items_confirms_the_unmeld_with_its_hooks() {
    // spec: HOOK-186 CLI-260 -- `--keep-items` still asks, defaulting to no;
    // the plan lists only the source's hook since the items stay.
    let sb = Sandbox::new("plan-un-keep");
    sb.meld_all();
    let r = sb.tty(&["unmeld", &sb.identity(), "--keep-items"], "\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("====== hooks: 1 ======"), "{}", r.stdout);
    assert!(r.stdout.contains("[y/s/r/N]"), "{}", r.stdout);
    assert!(sb.registered(), "Enter cancels: {}", r.stdout);
    let r = sb.tty(&["unmeld", &sb.identity(), "--keep-items"], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.ran("src-uninstall"), "{}", r.stdout);
    assert!(!sb.registered(), "{}", r.stdout);
    assert!(sb.installed("skills/widget"), "{}", r.stdout);
}

#[test]
fn unmeld_keep_items_with_yes_asks_a_hooks_only_plan_that_runs_by_default() {
    // spec: HOOK-186 HOOK-182 -- `--yes` answers the removal; the hooks-only
    // plan's default still runs the hooks.
    let sb = Sandbox::new("plan-un-keep-y");
    sb.meld_all();
    let r = sb.tty(&["unmeld", &sb.identity(), "--keep-items", "--yes"], "");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[Y/n/r]"), "{}", r.stdout);
    assert!(sb.ran("src-uninstall"), "{}", r.stdout);
    assert!(!sb.registered(), "{}", r.stdout);
}

#[test]
fn unmeld_of_a_hookless_single_item_source_still_confirms() {
    // spec: CLI-260 HOOK-186 -- no hook to list: the removal question alone,
    // `[y/N]`, and Enter declines.
    let sb = Sandbox::new("plan-un-bare");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ndescription = \"bare\"\n",
    );
    std::fs::remove_dir_all(sb.source.join("skills")).unwrap();
    sb.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    let r = sb.piped(&["meld", &sb.spec(), "--yes"], "");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    let r = sb.tty(&["unmeld", &sb.identity()], "\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[y/N]"), "{}", r.stdout);
    assert!(sb.registered() && sb.installed("rules/style.md"));
    let r = sb.tty(&["unmeld", &sb.identity()], "y\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(!sb.registered() && !sb.installed("rules/style.md"));
}

#[test]
fn unmeld_glob_with_yes_asks_one_hooks_only_plan_for_every_source() {
    // spec: HOOK-186 HOOK-182 -- a glob over two sources under `--yes`: one
    // plan lists both sources' uninstall hooks, asked once.
    let sb = Sandbox::new("multi-a");
    sb.meld_all();
    let other = sb.base.join("multi-b");
    write(
        &other.join("mind.toml"),
        &format!(
            "[[hooks]]\nname = \"b-teardown\"\nrun = \"{}\"\nevent = \"uninstall\"\n",
            sb.touch("b-uninstall")
        ),
    );
    write(&other.join("rules/b.md"), "---\ndescription: b\n---\n# b\n");
    git(&other, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&other, &["config", "user.email", "t@t"]);
    git(&other, &["config", "user.name", "t"]);
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "initial"]);
    let r = sb.piped(
        &["meld", &other.to_string_lossy(), "--yes", "--accept-hooks"],
        "",
    );
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    let glob = format!(
        "local/{}/multi-*",
        sb.base.file_name().unwrap().to_string_lossy()
    );
    let r = sb.tty(&["unmeld", &glob, "--yes"], "n\n");
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("====== hooks: 3 ======"), "{}", r.stdout);
    assert_eq!(r.stdout.matches("[Y/n/r]").count(), 1, "{}", r.stdout);
    assert!(r.stdout.contains("[3] b-teardown"), "{}", r.stdout);
    assert!(
        !sb.ran("src-uninstall") && !sb.ran("b-uninstall") && !sb.ran("item-uninstall"),
        "{}",
        r.stdout
    );
    assert!(!sb.registered(), "{}", r.stdout);
}

#[test]
fn unmeld_without_a_tty_or_yes_refuses() {
    // spec: CLI-259 CLI-260 HOOK-186
    let sb = Sandbox::new("plan-un-pipe");
    sb.meld_all();
    let r = sb.piped(&["unmeld", &sb.identity()], "y\n");
    assert!(!r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
    assert!(sb.registered() && sb.installed("skills/widget"));
    // `--keep-items`, and `--json` even at a terminal, refuse the same way.
    let r = sb.piped(&["unmeld", &sb.identity(), "--keep-items"], "");
    assert!(!r.success, "{}\n{}", r.stdout, r.stderr);
    let r = sb.tty(&["--json", "unmeld", &sb.identity()], "y\n");
    assert!(!r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(!sb.ran("src-uninstall") && !sb.ran("item-uninstall"));
    assert!(sb.registered() && sb.installed("skills/widget"));
}
