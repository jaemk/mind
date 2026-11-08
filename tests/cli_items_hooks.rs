//! Integration tests for items hooks: a source hook declared with
//! `event = "items"` that runs once per command with the source's installed
//! set (spec/install-hooks.md, "Items hooks").
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture (a local git repo melded by path, isolated MIND_HOME/CLAUDE_HOME).
//! Every items hook copies `$MIND_ITEMS_FILE` aside and appends to an order
//! log, so a test sees both what the hook was given and when it ran.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

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
    code: Option<i32>,
}

impl Run {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-ih-{}-{n}", std::process::id()));
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
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env("MIND_ITEMS_FILE", "/inherited/must/not/leak")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        }
    }

    /// Run `mind` at a simulated terminal (`MIND_TTY=1`, HOOK-109) with
    /// `stdin` as the replies.
    fn tty(&self, args: &[&str], stdin: &str) -> Run {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env("MIND_TTY", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn mind");
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
            code: out.status.code(),
        }
    }

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn remove_and_commit(&self, rel: &str) {
        std::fs::remove_dir_all(self.source.join(rel)).unwrap();
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "remove"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The registry identity a local-path meld gives this fixture (STO-13).
    fn source_id(&self) -> String {
        let owner = self.base.file_name().unwrap().to_string_lossy();
        let repo = self.source.file_name().unwrap().to_string_lossy();
        format!("local/{owner}/{repo}")
    }

    fn skill(&self, name: &str, extra_frontmatter: &str) {
        write(
            &self.source.join(format!("skills/{name}/SKILL.md")),
            &format!("---\ndescription: {name}\n{extra_frontmatter}---\nbody of {name}\n"),
        );
    }

    /// A command appending `word` to the order log.
    fn log(&self, word: &str) -> String {
        format!("echo {word} >> {}", self.base.join("order").display())
    }

    /// The items hook: copy the document aside, then log `items`.
    fn items_hook_cmd(&self) -> String {
        format!(
            "cp \\\"$MIND_ITEMS_FILE\\\" {} && {}",
            self.base.join("last.json").display(),
            self.log("items")
        )
    }

    /// Commit a `mind.toml` declaring the items hook plus `extra` TOML.
    fn declare(&self, extra: &str) {
        self.write_and_commit(
            "mind.toml",
            &format!(
                "[[hooks]]\nrun = \"{}\"\nname = \"registry\"\nevent = \"items\"\n{extra}",
                self.items_hook_cmd()
            ),
        );
    }

    fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("order"))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn runs(&self) -> usize {
        self.order().iter().filter(|l| *l == "items").count()
    }

    /// The last items document the hook was given.
    fn doc(&self) -> Value {
        let text = std::fs::read_to_string(self.base.join("last.json")).expect("hook ran");
        serde_json::from_str(&text).expect("the items document is JSON")
    }

    fn registry(&self) -> Value {
        let text = std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap();
        serde_json::from_str(&text).unwrap()
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

/// The effective names in one of the document's lists.
fn names(doc: &Value, list: &str) -> Vec<String> {
    doc[list]
        .as_array()
        .unwrap_or_else(|| panic!("{list} is a list: {doc}"))
        .iter()
        .map(|e| e["effective_name"].as_str().unwrap().to_string())
        .collect()
}

/// A fixture with skills `alpha` and `beta` and the items hook, melded without
/// installing anything.
fn registered(name: &str) -> Sandbox {
    let sb = Sandbox::new(name);
    sb.skill("alpha", "");
    sb.skill("beta", "");
    sb.declare("");
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    sb
}

// ---------------------------------------------------------------------------
// When it fires and what it is given (HOOK-191, HOOK-193, HOOK-195)
// ---------------------------------------------------------------------------

/// Each command that changes the installed set runs the hook once, with the
/// difference against the set it last applied and the full installed set.
#[test]
fn learn_and_forget_run_the_hook_with_the_difference_and_the_installed_set() {
    // spec: HOOK-191 HOOK-193 HOOK-195
    let sb = registered("ih-learn");
    assert_eq!(sb.runs(), 0, "a register-only meld installs nothing");

    let r = sb.mind(&["learn", "alpha", "--accept-hooks=items"]);
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
    assert!(
        r.stdout.contains("running items hook 'registry' for ")
            && r.stdout.contains("(1 added, 0 updated, 0 removed)"),
        "{}",
        r.stdout
    );
    let doc = sb.doc();
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["source"], sb.source_id());
    assert_eq!(doc["command"], "learn");
    assert!(
        doc["commit"].as_str().is_some_and(|c| c.len() >= 7),
        "{doc}"
    );
    assert_eq!(names(&doc, "added"), ["alpha"]);
    assert_eq!(names(&doc, "installed"), ["alpha"]);
    let entry = &doc["installed"][0];
    assert_eq!(entry["kind"], "skill");
    assert_eq!(entry["name"], "alpha");
    let store = entry["store"].as_str().unwrap();
    assert!(
        Path::new(store).is_absolute() && Path::new(store).join("SKILL.md").exists(),
        "store is the absolute installed path: {store}"
    );
    assert!(entry["profiles"].as_array().is_some(), "{entry}");

    let r = sb.mind(&["learn", "beta", "--accept-hooks=items"]);
    assert!(r.ok(), "learn: {}", r.stderr);
    let doc = sb.doc();
    assert_eq!(names(&doc, "added"), ["beta"], "only what changed since");
    assert_eq!(names(&doc, "installed"), ["alpha", "beta"]);

    let r = sb.mind(&["--yes", "forget", "alpha", "--accept-hooks=items"]);
    assert!(r.ok(), "forget: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 3);
    let doc = sb.doc();
    assert_eq!(doc["command"], "forget");
    assert_eq!(names(&doc, "removed"), ["alpha"]);
    assert!(names(&doc, "added").is_empty());
    assert_eq!(names(&doc, "installed"), ["beta"]);

    // A command that changes nothing offers nothing.
    let r = sb.mind(&["learn", "beta", "--accept-hooks=items"]);
    assert!(r.ok(), "learn: {}", r.stderr);
    assert_eq!(sb.runs(), 3, "an unchanged set runs no items hook");
}

/// A whole-source meld runs the hook once, after every item installed, with
/// all of them added.
#[test]
fn meld_runs_the_hook_once_after_installing_everything() {
    // spec: HOOK-191 HOOK-192
    let sb = Sandbox::new("ih-meld");
    sb.skill("alpha", &format!("install: {}\n", sb.log("item-alpha")));
    sb.skill("beta", &format!("install: {}\n", sb.log("item-beta")));
    sb.declare(&format!("\n[[hooks]]\nrun = \"{}\"\n", sb.log("source")));

    let r = sb.mind(&["--yes", "meld", &sb.source_spec(), "--accept-hooks"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
    assert_eq!(
        sb.order(),
        ["source", "item-alpha", "item-beta", "items"],
        "source.install -> item.install* -> items"
    );
    let doc = sb.doc();
    assert_eq!(doc["command"], "meld");
    assert_eq!(names(&doc, "added"), ["alpha", "beta"]);
}

/// A content-only change makes the hook pending, and `upgrade` lists the item
/// under `updated`; an item the source drops goes under `removed`.
#[test]
fn upgrade_reports_content_changes_as_updated() {
    // spec: HOOK-191 HOOK-193
    let sb = registered("ih-upgrade");
    assert!(sb.mind(&["learn", "alpha", "--accept-hooks=items"]).ok());
    assert!(sb.mind(&["learn", "beta", "--accept-hooks=items"]).ok());
    assert_eq!(sb.runs(), 2);

    sb.skill("alpha", "note: v2\n");
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "alpha v2"]);
    let r = sb.mind(&["--yes", "upgrade", "--accept-hooks"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 3, "{}", r.stdout);
    let doc = sb.doc();
    assert_eq!(doc["command"], "upgrade");
    assert_eq!(names(&doc, "updated"), ["alpha"]);
    assert!(names(&doc, "added").is_empty() && names(&doc, "removed").is_empty());

    // An upgrade with nothing to do offers nothing.
    let r = sb.mind(&["--yes", "upgrade", "--accept-hooks"]);
    assert!(r.ok(), "upgrade: {}", r.stderr);
    assert_eq!(sb.runs(), 3, "nothing pending: {}", r.stdout);
}

/// Without a terminal or `--accept-hooks=items` the hook is skipped with a
/// remedy and stays pending; the next `upgrade` offers it with every change
/// since the last applied set, even though that upgrade changed nothing.
#[test]
fn a_skipped_hook_stays_pending_and_upgrade_catches_it_up() {
    // spec: HOOK-191 HOOK-194 HOOK-195
    let sb = registered("ih-skip");
    let id = sb.source_id();
    let r = sb.mind(&["learn", "alpha"]);
    assert!(
        r.ok(),
        "a skipped items hook does not fail the learn: {}",
        r.stderr
    );
    assert_eq!(sb.runs(), 0);
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(
        all.contains("skipped items hook 'registry'")
            && all.contains(&format!(
                "mind hooks run '{id}' --event items --accept-hooks=items"
            )),
        "{all}"
    );
    // `--accept-hooks=install` does not accept an items hook.
    assert!(sb.mind(&["learn", "beta", "--accept-hooks=install"]).ok());
    assert_eq!(sb.runs(), 0);

    let r = sb.mind(&["--yes", "upgrade", "--no-sync", "--accept-hooks=items"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
    let doc = sb.doc();
    assert_eq!(
        names(&doc, "added"),
        ["alpha", "beta"],
        "both changes since"
    );
    assert_eq!(doc["command"], "upgrade");
}

/// `mind upgrade <source>` (a positional naming a source, CLI-262) catches up
/// a pending hook the same as `--source <source>`.
#[test]
fn upgrade_with_a_positional_source_catches_up_a_pending_hook() {
    // spec: HOOK-191 CLI-262
    let sb = registered("ih-upgrade-pos");
    assert!(sb.mind(&["learn", "alpha"]).ok());
    assert_eq!(sb.runs(), 0, "skipped without a terminal");

    let r = sb.mind(&[
        "--yes",
        "upgrade",
        &sb.source_id(),
        "--no-sync",
        "--accept-hooks=items",
    ]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
    assert_eq!(names(&sb.doc(), "added"), ["alpha"]);
}

/// An unmeld that removes no items still offers a hook left pending by an
/// earlier skipped run, so the removals reach it before the source goes.
#[test]
fn unmeld_delivers_removals_an_earlier_skip_left_pending() {
    // spec: HOOK-192 HOOK-195
    let sb = registered("ih-unmeld-pending");
    assert!(sb.mind(&["learn", "alpha", "--accept-hooks=items"]).ok());
    assert_eq!(sb.runs(), 1);
    let r = sb.mind(&["--yes", "forget", "alpha"]);
    assert!(r.ok(), "forget: {}", r.stderr);
    assert_eq!(sb.runs(), 1, "skipped without a terminal");

    let r = sb.mind(&["--yes", "unmeld", &sb.source_id(), "--accept-hooks"]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 2, "{}", r.stdout);
    let doc = sb.doc();
    assert_eq!(doc["command"], "unmeld");
    assert_eq!(names(&doc, "removed"), ["alpha"]);
}

/// A failing items hook exits 3, keeps the command's item changes, and leaves
/// the hook pending, so the next offer gives it the same difference again.
#[test]
fn a_failed_hook_keeps_the_items_and_stays_pending() {
    // spec: HOOK-195
    let sb = Sandbox::new("ih-fail");
    sb.skill("alpha", "");
    let flag = sb.base.join("fail");
    write(&flag, "");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[[hooks]]\nrun = \"{} && test ! -e {}\"\nevent = \"items\"\n",
            sb.items_hook_cmd(),
            flag.display()
        ),
    );
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .ok()
    );

    let r = sb.mind(&["learn", "alpha", "--accept-hooks=items"]);
    assert_eq!(
        r.code,
        Some(3),
        "a failed hook exits 3: {}\n{}",
        r.stdout,
        r.stderr
    );
    assert!(
        sb.claude_home.join("skills/alpha/SKILL.md").exists(),
        "the learn's install stands"
    );
    let rec = &sb.registry()["sources"][0]["items_hooks"];
    assert!(
        rec.as_array()
            .is_none_or(|a| a.iter().all(|r| r["applied"].is_null())),
        "a failure records no applied set: {rec}"
    );

    std::fs::remove_file(&flag).unwrap();
    let r = sb.mind(&[
        "hooks",
        "run",
        &sb.source_id(),
        "--event",
        "items",
        "--accept-hooks=items",
    ]);
    assert!(r.ok(), "hooks run: {}\n{}", r.stdout, r.stderr);
    let doc = sb.doc();
    assert_eq!(doc["command"], "hooks run");
    assert_eq!(names(&doc, "added"), ["alpha"], "the same difference again");
}

// ---------------------------------------------------------------------------
// Ordering at unmeld (HOOK-192)
// ---------------------------------------------------------------------------

/// At `unmeld` the items hook runs after the items' uninstall hooks, with an
/// empty installed set, and before the source's uninstall hooks.
#[test]
fn unmeld_runs_the_hook_between_item_and_source_uninstall_hooks() {
    // spec: HOOK-192
    let sb = Sandbox::new("ih-unmeld");
    sb.skill(
        "alpha",
        &format!("uninstall: {}\n", sb.log("item-uninstall")),
    );
    sb.declare(&format!(
        "\n[[hooks]]\nrun = \"{}\"\nevent = \"uninstall\"\n",
        sb.log("source-uninstall")
    ));
    assert!(
        sb.mind(&["--yes", "meld", &sb.source_spec(), "--accept-hooks"])
            .ok()
    );
    assert_eq!(sb.order(), ["items"]);

    let r = sb.mind(&["--yes", "unmeld", &sb.source_id(), "--accept-hooks"]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.order(),
        ["items", "item-uninstall", "items", "source-uninstall"],
        "item.uninstall* -> items -> source.uninstall"
    );
    let doc = sb.doc();
    assert_eq!(doc["command"], "unmeld");
    assert_eq!(names(&doc, "removed"), ["alpha"]);
    assert!(names(&doc, "installed").is_empty());
}

/// `unmeld --keep-items` removes no items and runs no items hook.
#[test]
fn unmeld_keep_items_runs_no_items_hook() {
    // spec: HOOK-191
    let sb = Sandbox::new("ih-keep");
    sb.skill("alpha", "");
    sb.declare("");
    assert!(
        sb.mind(&["--yes", "meld", &sb.source_spec(), "--accept-hooks"])
            .ok()
    );
    let r = sb.mind(&[
        "--yes",
        "unmeld",
        &sb.source_id(),
        "--keep-items",
        "--accept-hooks",
    ]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs(), 1, "only the meld's run");
}

// ---------------------------------------------------------------------------
// The unified plan (HOOK-198)
// ---------------------------------------------------------------------------

/// The meld plan lists the items hook last, after the source and item hooks,
/// and `y` runs it with no prompt of its own.
#[test]
fn the_meld_plan_lists_the_items_hook_last_and_its_answer_runs_it() {
    // spec: HOOK-198 HOOK-181
    let sb = Sandbox::new("ih-plan-meld");
    sb.skill("alpha", &format!("install: {}\n", sb.log("item-alpha")));
    sb.declare(&format!(
        "\n[[hooks]]\nrun = \"{}\"\nname = \"setup\"\n",
        sb.log("source")
    ));

    let r = sb.tty(&["meld", &sb.source_spec()], "y\n");
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let out = &r.stdout;
    assert!(out.contains("====== hooks: 3 ======"), "{out}");
    assert!(out.contains("[1] setup"), "{out}");
    assert!(
        out.contains("[3] registry\n    Event:   items (required)"),
        "the items hook is listed last: {out}"
    );
    assert!(
        !out.contains("====== hook: "),
        "no prompt of its own: {out}"
    );
    assert_eq!(sb.order(), ["source", "item-alpha", "items"]);
}

/// `s` skips the listed items hook like the rest: it stays pending.
#[test]
fn skipping_the_plan_skips_the_items_hook() {
    // spec: HOOK-198 HOOK-181 HOOK-195
    let sb = registered("ih-plan-skip");
    let r = sb.tty(&["learn", "alpha"], "s\n");
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[1] registry"), "{}", r.stdout);
    assert!(sb.claude_home.join("skills/alpha/SKILL.md").exists());
    assert_eq!(sb.runs(), 0, "{}", r.stdout);
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);

    let r = sb.mind(&["hooks", "list", &sb.source_id()]);
    assert!(r.stdout.contains("pending (1 added"), "{}", r.stdout);
}

/// `r` prompts the items hook on its own at its run point.
#[test]
fn reviewing_the_plan_prompts_the_items_hook_on_its_own() {
    // spec: HOOK-198 HOOK-181
    let sb = registered("ih-plan-review");
    let r = sb.tty(&["learn", "alpha"], "r\ny\n");
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("[1] registry"), "listed: {}", r.stdout);
    assert_eq!(r.stdout.matches("====== hook: ").count(), 1, "{}", r.stdout);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
}

/// An `upgrade` that changes nothing lists a hook an earlier run left pending
/// in a hooks-only plan, and its answer runs it.
#[test]
fn upgrade_lists_a_pending_items_hook_in_a_hooks_only_plan() {
    // spec: HOOK-198 HOOK-182
    let sb = registered("ih-plan-upgrade");
    assert!(sb.mind(&["learn", "alpha"]).ok());
    assert_eq!(sb.runs(), 0, "skipped without a terminal");

    let r = sb.tty(&["upgrade", "--no-sync"], "y\n");
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("[1] registry") && r.stdout.contains("[Y/n/r]"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("====== hook: "), "{}", r.stdout);
    assert_eq!(sb.runs(), 1, "{}", r.stdout);
    assert_eq!(sb.doc()["command"], "upgrade");

    // Applied now: the next upgrade lists nothing and asks nothing.
    let r = sb.tty(&["upgrade", "--no-sync"], "");
    assert!(r.ok(), "upgrade: {}", r.stderr);
    assert!(!r.stdout.contains("registry"), "{}", r.stdout);
}

/// The unmeld plan lists the items hook between the items' uninstall hooks
/// and the source's, and its answer runs them in that order.
#[test]
fn the_unmeld_plan_lists_the_items_hook_between_the_uninstall_hooks() {
    // spec: HOOK-198 HOOK-186 HOOK-192
    let sb = Sandbox::new("ih-plan-unmeld");
    sb.skill(
        "alpha",
        &format!("uninstall: {}\n", sb.log("item-uninstall")),
    );
    sb.declare(&format!(
        "\n[[hooks]]\nrun = \"{}\"\nname = \"teardown\"\nevent = \"uninstall\"\n",
        sb.log("source-uninstall")
    ));
    assert!(
        sb.mind(&["--yes", "meld", &sb.source_spec(), "--accept-hooks"])
            .ok()
    );

    let r = sb.tty(&["unmeld", &sb.source_id()], "y\n");
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    let out = &r.stdout;
    assert!(out.contains("====== hooks: 3 ======"), "{out}");
    assert!(out.contains("[2] registry"), "{out}");
    assert!(out.contains("[3] teardown"), "{out}");
    assert!(!out.contains("====== hook: "), "{out}");
    assert_eq!(
        sb.order(),
        ["items", "item-uninstall", "items", "source-uninstall"]
    );
}

// ---------------------------------------------------------------------------
// On demand and disclosure (HOOK-196)
// ---------------------------------------------------------------------------

/// `hooks run --event items` runs only a pending hook, says why when nothing
/// is pending, runs anyway under `--force`, and refuses an item target.
#[test]
fn hooks_run_items_runs_pending_hooks_and_force_reruns() {
    // spec: HOOK-196
    let sb = registered("ih-run");
    let id = sb.source_id();
    assert!(sb.mind(&["learn", "alpha", "--accept-hooks=items"]).ok());
    assert_eq!(sb.runs(), 1);

    let run = |extra: &[&str]| {
        let mut args = vec![
            "hooks",
            "run",
            id.as_str(),
            "--event",
            "items",
            "--accept-hooks=items",
        ];
        args.extend_from_slice(extra);
        sb.mind(&args)
    };
    let r = run(&[]);
    assert!(r.ok(), "{}", r.stderr);
    assert_eq!(sb.runs(), 1, "nothing pending");
    assert!(
        format!("{}{}", r.stdout, r.stderr).contains("nothing pending"),
        "{}{}",
        r.stdout,
        r.stderr
    );

    let r = run(&["--force"]);
    assert!(r.ok(), "{}", r.stderr);
    assert_eq!(sb.runs(), 2);
    let doc = sb.doc();
    assert_eq!(doc["command"], "hooks run");
    assert!(names(&doc, "added").is_empty(), "nothing changed: {doc}");
    assert_eq!(names(&doc, "installed"), ["alpha"]);

    let r = sb.mind(&["hooks", "run", &format!("{id}#alpha"), "--event", "items"]);
    assert!(!r.ok());
    assert!(
        r.stderr
            .contains("--event items is valid only for a source target"),
        "{}",
        r.stderr
    );
}

/// `hooks list` shows an items hook with its applied/pending state, `review`
/// discloses it, and `recall --sources` counts it.
#[test]
fn list_review_and_recall_disclose_the_items_hook() {
    // spec: HOOK-196
    let sb = registered("ih-list");
    let id = sb.source_id();
    let r = sb.mind(&["learn", "alpha"]);
    assert!(r.ok());

    let r = sb.mind(&["hooks", "list", &id]);
    assert!(r.ok(), "{}", r.stderr);
    assert!(
        r.stdout.contains("[items] required")
            && r.stdout
                .contains("pending (1 added, 0 updated, 0 removed; 0 items applied)"),
        "{}",
        r.stdout
    );

    assert!(
        sb.mind(&[
            "hooks",
            "run",
            &id,
            "--event",
            "items",
            "--accept-hooks=items"
        ])
        .ok()
    );
    let r = sb.mind(&["--json", "hooks", "list", &id]);
    assert!(r.ok(), "{}", r.stderr);
    let list: Value = serde_json::from_str(&r.stdout).expect("one JSON document");
    let hook = list["sources"][0]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["event"] == "items")
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(hook["status"], "applied (1 items)");

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        format!("{}{}", r.stdout, r.stderr)
            .contains("source declares a required items hook 'registry'"),
        "{}{}",
        r.stdout,
        r.stderr
    );

    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.ok(), "{}", r.stderr);
    assert!(r.stdout.contains(" hook"), "{}", r.stdout);
}

/// Under `--json` the verb's one document carries the items hook's result.
#[test]
fn json_results_include_the_items_hook() {
    // spec: HOOK-194
    let sb = registered("ih-json");
    let r = sb.mind(&["--json", "learn", "alpha", "--accept-hooks=items"]);
    assert!(r.ok(), "{}", r.stderr);
    let doc: Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("one JSON document: {e}\n{}", r.stdout));
    let hooks = doc["hooks"].as_array().unwrap_or_else(|| panic!("{doc}"));
    assert!(
        hooks
            .iter()
            .any(|h| h["event"] == "items" && h["outcome"] == "ok"),
        "{doc}"
    );
}

/// A curator's `[[discover.sources.hooks]]` items entry is recorded on the
/// nested source (which has no `mind.toml` to declare it) and runs when that
/// source's installed set changes.
#[test]
fn a_curated_items_hook_runs_for_the_nested_source() {
    // spec: HOOK-190
    let nested = Sandbox::new("ih-nested");
    nested.skill("alpha", "");
    git(&nested.source, &["add", "-A"]);
    git(&nested.source, &["commit", "-qm", "alpha"]);
    let curator = Sandbox::new("ih-curator");
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n\n\
             [[discover.sources.hooks]]\nrun = \"{}\"\nname = \"curated\"\nevent = \"items\"\n",
            nested.source_spec(),
            curator.items_hook_cmd()
        ),
    );
    let r = curator.mind(&["meld", &curator.source_spec(), "--accept-hooks"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(curator.runs(), 0, "the nested source installed nothing yet");

    let r = curator.mind(&["learn", "alpha", "--accept-hooks=items"]);
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    assert_eq!(curator.runs(), 1, "{}", r.stdout);
    assert!(
        r.stdout.contains("running items hook 'curated'"),
        "{}",
        r.stdout
    );
    let doc = curator.doc();
    assert_eq!(doc["source"], nested.source_id());
    assert_eq!(names(&doc, "added"), ["alpha"]);

    let r = curator.mind(&["hooks", "list", &nested.source_id()]);
    assert!(r.ok(), "{}", r.stderr);
    assert!(r.stdout.contains("[items] required"), "{}", r.stdout);
}

/// A source that drops an item: the upgrade's removal shows under `removed`.
#[test]
fn an_item_the_source_drops_is_removed_for_a_pruning_source() {
    // spec: HOOK-191 HOOK-193
    let sb = Sandbox::new("ih-prune");
    sb.skill("alpha", "");
    sb.skill("beta", "");
    sb.declare("\n[source]\nprune = true\n");
    assert!(
        sb.mind(&["--yes", "meld", &sb.source_spec(), "--accept-hooks"])
            .ok()
    );
    sb.remove_and_commit("skills/beta");
    let r = sb.mind(&["--yes", "upgrade", "--accept-hooks"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    let doc = sb.doc();
    assert_eq!(names(&doc, "removed"), ["beta"], "{doc}");
    assert_eq!(names(&doc, "installed"), ["alpha"]);
}
