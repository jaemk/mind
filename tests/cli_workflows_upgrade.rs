//! Integration tests for the workflow warnings `upgrade` emits (spec/workflows.md
//! WF-24, WF-29, WF-50). Each test drives the real `mind` binary against a
//! hermetic fixture: a local git repo melded by filesystem path, with
//! MIND_HOME/CLAUDE_HOME pointed at temp dirs. No network.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---- Sandbox helpers --------------------------------------------------------

struct Sandbox {
    base: PathBuf,
    /// The source repo `mind` melds (by filesystem path) and later syncs from.
    src: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-wfup-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("wf-source");
        let sb = Sandbox {
            base: base.clone(),
            src: src.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        git_init(&src);
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_ABSORB_TO")
            // The assertions below read lobe link paths, so a stray
            // MIND_AGENT_HOMES would move them, and the over-cap fixture is
            // sized against the DEFAULT metadata cap, so a stray
            // MIND_MAX_METADATA_SIZE would change what it proves.
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

    /// `mind` with a faked TTY (HOOK-109) and piped stdin, so a lifecycle hook
    /// reaches its consent prompt instead of being skipped as it is in a
    /// non-interactive context.
    fn mind_interactive(&self, args: &[&str], stdin: &str) -> Run {
        use std::io::Write as _;
        let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env("MIND_TTY", "1")
            .env_remove("MIND_ABSORB_TO")
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_MAX_METADATA_SIZE")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn mind");
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("wait for mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn src_spec(&self) -> String {
        self.src.to_string_lossy().into_owned()
    }

    /// Write `workflows/<name>.js` in the source repo with the given harness
    /// name in its `meta` object.
    fn write_workflow(&self, name: &str, meta_name: &str) {
        write_file(
            &self.src.join("workflows").join(format!("{name}.js")),
            &workflow_js(meta_name, name),
        );
    }

    /// Write `workflows/<name>.js` with arbitrary contents (a `meta` mind
    /// cannot read, a missing key, a padded over-cap file, ...).
    fn write_workflow_raw(&self, name: &str, body: &str) {
        write_file(&self.src.join("workflows").join(format!("{name}.js")), body);
    }

    fn commit_src(&self, msg: &str) {
        git(&self.src, &["add", "-A"]);
        git(&self.src, &["commit", "-qm", msg]);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

// ---- filesystem helpers -----------------------------------------------------

fn workflow_js(meta_name: &str, description_of: &str) -> String {
    format!(
        "export const meta = {{\n  \
         name: '{meta_name}',\n  \
         description: 'Run the {description_of} pass',\n  \
         phases: [{{ title: 'Go' }}],\n\
         }}\n\
         \n\
         phase('Go')\n\
         const done = await agent('Do the {description_of} work.')\n\
         \n\
         return done\n"
    )
}

fn write_file(path: &Path, contents: &str) {
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

fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), "# wf-source\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-qm", "init"]);
}

// ---- WF-24 / WF-29: `upgrade` is a warning site, not just `learn` -----------

/// An upstream edit that makes an installed workflow's `meta.name` diverge from
/// its item name -- and collide with a second installed workflow's harness name
/// -- is reported by the `upgrade` that applies it. The warnings cannot come
/// from `learn`: at install time the two names agreed, so the learn run is
/// asserted clean. Only the upgrade pass can surface the new defect, and it is
/// advisory: the upgrade still succeeds.
///
/// The workflow file is a plain `.js` with no `{{ns:}}` token, so the store copy
/// the warning reads carries the literal name the harness will see (WF-50: a
/// workflow is a one-file kind at `workflows/<name>.js`).
// spec: WF-24 WF-29 WF-50
#[test]
fn wf24_wf29_upgrade_warns_on_newly_diverged_and_colliding_meta_name() {
    let sb = Sandbox::new();
    sb.write_workflow("alpha", "alpha");
    sb.write_workflow("beta", "beta");
    sb.commit_src("add workflows");

    let spec = sb.src_spec();
    let meld = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(
        meld.success,
        "meld must succeed: stdout={} stderr={}",
        meld.stdout, meld.stderr
    );

    let learn_a = sb.mind(&["learn", "workflow:alpha"]);
    assert!(
        learn_a.success,
        "learn workflow:alpha must succeed: stdout={} stderr={}",
        learn_a.stdout, learn_a.stderr
    );
    let learn_b = sb.mind(&["learn", "workflow:beta"]);
    assert!(
        learn_b.success,
        "learn workflow:beta must succeed: stdout={} stderr={}",
        learn_b.stdout, learn_b.stderr
    );
    // spec: WF-24 -- at install time the names agree, so nothing is reported.
    // This is what makes the upgrade assertions below load-bearing.
    assert!(
        !learn_a.stderr.contains("the harness resolves it as"),
        "a workflow whose meta.name matches its item name must draw no \
         divergence warning at learn: {}",
        learn_a.stderr
    );
    assert!(
        !learn_b.stderr.contains("it answers to the harness name"),
        "two workflows with distinct harness names must draw no collision \
         warning at learn: {}",
        learn_b.stderr
    );

    // Upstream renames alpha's harness name onto beta's.
    sb.write_workflow("alpha", "beta");
    sb.commit_src("alpha now answers to beta");

    let sync = sb.mind(&["sync"]);
    assert!(
        sync.success,
        "sync must succeed: stdout={} stderr={}",
        sync.stdout, sync.stderr
    );

    let up = sb.mind(&["upgrade", "--yes"]);
    // spec: WF-24 WF-29 -- advisory only: the upgrade is neither failed nor
    // altered by either warning.
    assert!(
        up.success,
        "upgrade must succeed despite the warnings: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    assert!(
        up.stdout.contains("upgraded"),
        "the upgrade must actually have applied: stdout={} stderr={}",
        up.stdout,
        up.stderr
    );
    // spec: WF-24 -- the divergence, reported by `upgrade`, keyed to the item.
    assert!(
        up.stderr.contains("workflow:alpha"),
        "the upgrade warnings must name the upgraded item: {}",
        up.stderr
    );
    assert!(
        up.stderr
            .contains("the harness resolves it as 'beta', not 'alpha'"),
        "upgrade must report the WF-24 divergence introduced upstream: {}",
        up.stderr
    );
    // spec: WF-29 -- and the collision with the already-installed sibling.
    assert!(
        up.stderr
            .contains("it answers to the harness name 'beta', which workflow:beta also claims"),
        "upgrade must report the WF-29 collision against the whole installed \
         set, not just the items it touched: {}",
        up.stderr
    );
}

/// When the upgrade is a rename (the source gained a prefix, so the effective
/// name changed), the warnings are reported under the NEW key. The old manifest
/// entry is gone by the time they run, so a report keyed to the old name would
/// silently find nothing and warn about nothing.
// spec: WF-24 WF-50
#[test]
fn wf24_upgrade_reports_a_renamed_workflow_under_its_new_key() {
    let sb = Sandbox::new();
    sb.write_workflow("alpha", "alpha");
    sb.commit_src("add workflow");

    let spec = sb.src_spec();
    let meld = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(
        meld.success,
        "meld must succeed: stdout={} stderr={}",
        meld.stdout, meld.stderr
    );
    let learn = sb.mind(&["learn", "workflow:alpha"]);
    assert!(
        learn.success,
        "learn workflow:alpha must succeed: stdout={} stderr={}",
        learn.stdout, learn.stderr
    );

    // The source declares a prefix: every item installs as `labs:<name>`, so
    // `alpha` becomes `labs:alpha` while its `meta.name` stays `alpha`.
    write_file(
        &sb.src.join("mind.toml"),
        "[source]\ndescription = \"workflow fixture\"\nprefix = \"labs\"\n",
    );
    sb.commit_src("namespace the source");

    let sync = sb.mind(&["sync"]);
    assert!(
        sync.success,
        "sync must succeed: stdout={} stderr={}",
        sync.stdout, sync.stderr
    );

    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        up.success,
        "upgrade must succeed: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    // spec: WF-24 -- keyed to the post-rename identity.
    assert!(
        up.stderr.contains("workflow:labs:alpha"),
        "the warning must be keyed to the new name, not the pre-rename one: {}",
        up.stderr
    );
    assert!(
        up.stderr
            .contains("the harness resolves it as 'alpha', not 'labs:alpha'"),
        "upgrade must report the divergence a prefix change introduces: {}",
        up.stderr
    );
    // spec: WF-24 NS-11 -- the remedy token names the BARE name, which is the
    // only spelling `{{ns:}}` resolves. This is the surface where getting it
    // wrong is most tempting and least visible: the item is keyed, stored, and
    // reported under `labs:alpha` by this point, so a remedy built from the
    // name in the message would tell the author to write a token that resolves
    // to no sibling and turns an advisory warning into a hard `bad-reference`
    // (NS-12) on the next install.
    assert!(
        up.stderr.contains("`meta.name: '{{ns:alpha}}'`"),
        "the remedy must be the token that actually resolves: {}",
        up.stderr
    );
    assert!(
        !up.stderr.contains("{{ns:labs:alpha}}"),
        "a prefixed referent names no sibling (NS-11): {}",
        up.stderr
    );
}

// ---- CLI-217: the warnings survive `--json` -------------------------------

/// `upgrade --json --yes` still emits the workflow warnings, on stderr, leaving
/// stdout holding exactly one JSON document (CLI-217). The `warn_workflows`
/// call sits ahead of the `--json` return, so a caller scripting upgrades is
/// not the one caller who silently stops hearing about a shadowed harness name.
// spec: WF-24 WF-29 CLI-217
#[test]
fn wf24_wf29_upgrade_warnings_reach_stderr_under_json() {
    let sb = Sandbox::new();
    sb.write_workflow("alpha", "alpha");
    sb.write_workflow("beta", "beta");
    sb.commit_src("add workflows");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    assert!(sb.mind(&["learn", "workflow:alpha"]).success);
    assert!(sb.mind(&["learn", "workflow:beta"]).success);

    sb.write_workflow("alpha", "beta");
    sb.commit_src("alpha now answers to beta");
    assert!(sb.mind(&["sync"]).success);

    let up = sb.mind(&["upgrade", "--yes", "--json"]);
    assert!(
        up.success,
        "upgrade --json must succeed: stdout={} stderr={}",
        up.stdout, up.stderr
    );

    // spec: CLI-217 -- stdout is the document and nothing else. Parsing is the
    // assertion: a warning printed to stdout would leave prose before `{`.
    let doc: serde_json::Value = serde_json::from_str(up.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document: {e}\n{}", up.stdout));
    assert_eq!(
        doc["action"].as_str(),
        Some("upgrade"),
        "the document must answer the invoked verb: {}",
        up.stdout
    );
    assert!(
        !up.stdout.contains("warning:"),
        "no warning may land on stdout under --json: {}",
        up.stdout
    );

    // spec: WF-24 WF-29 -- and both warnings are still reported, on stderr.
    assert!(
        up.stderr
            .contains("the harness resolves it as 'beta', not 'alpha'"),
        "the WF-24 divergence must survive --json (on stderr): {}",
        up.stderr
    );
    assert!(
        up.stderr
            .contains("it answers to the harness name 'beta', which workflow:beta also claims"),
        "the WF-29 collision must survive --json (on stderr): {}",
        up.stderr
    );
}

// ---- WF-30 / WF-31: the unloadable warnings on the upgrade path -----------

/// An upstream edit that makes an installed workflow unloadable is reported by
/// the `upgrade` that applies it, on the same terms `learn` reports it on:
/// warn, install anyway (WF-31). Two shapes in one batch -- a `meta` mind can
/// read but that is missing `description`, and a file with no `meta` at all --
/// because they take different branches of `skip_reasons`.
///
/// The file with no readable `meta` must draw the unloadable warning and NOT a
/// WF-24 divergence: it has no harness name to diverge, and reporting one would
/// be a second complaint about the same defect.
// spec: WF-30 WF-31
#[test]
fn wf30_wf31_upgrade_warns_when_an_edit_makes_a_workflow_unloadable() {
    let sb = Sandbox::new();
    sb.write_workflow("gamma", "gamma");
    sb.write_workflow("delta", "delta");
    sb.commit_src("add workflows");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    let learn_g = sb.mind(&["learn", "workflow:gamma"]);
    assert!(learn_g.success, "learn gamma: {}", learn_g.stderr);
    let learn_d = sb.mind(&["learn", "workflow:delta"]);
    assert!(learn_d.success, "learn delta: {}", learn_d.stderr);
    // The install run is clean, so the upgrade assertions below are the ones
    // doing the work.
    assert!(
        !learn_g.stderr.contains("will not load this workflow")
            && !learn_d.stderr.contains("will not load this workflow"),
        "a loadable workflow must draw no WF-30 warning at learn: {} {}",
        learn_g.stderr,
        learn_d.stderr
    );

    // gamma keeps a readable `meta` but loses its `description`; delta loses
    // the `meta` object entirely.
    sb.write_workflow_raw("gamma", "export const meta = {\n  name: 'gamma',\n}\n");
    sb.write_workflow_raw("delta", "// no meta here at all\nconst x = 1\n");
    sb.commit_src("break both workflows");
    assert!(sb.mind(&["sync"]).success);

    let up = sb.mind(&["upgrade", "--yes"]);
    // spec: WF-31 -- advisory: the upgrade still applies.
    assert!(
        up.success,
        "upgrade must succeed despite the WF-30 findings: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    assert!(
        up.stderr.contains(
            "workflow:gamma: the harness will not load this workflow: `meta.description` is \
             missing; installed anyway"
        ),
        "upgrade must report the missing `meta.description`: {}",
        up.stderr
    );
    assert!(
        up.stderr.contains(
            "workflow:delta: the harness will not load this workflow: mind read no `name`, \
             `description`, or `whenToUse` from its `meta`; installed anyway"
        ),
        "upgrade must report the unreadable `meta`, in terms of what mind read: {}",
        up.stderr
    );
    // spec: WF-24 WF-30 -- no name, so no divergence complaint on top.
    assert!(
        !up.stderr.contains("the harness resolves it as"),
        "an unloadable workflow must not also draw a divergence warning: {}",
        up.stderr
    );
    // spec: WF-31 -- and the new (broken) content is what is installed.
    let installed = std::fs::read_to_string(sb.claude_home.join("workflows/delta.js"))
        .expect("the link must still resolve after the upgrade");
    assert!(
        installed.contains("no meta here at all"),
        "the upgrade must have applied the unloadable version: {installed}"
    );
}

/// A workflow that grows past the harness's 524288-byte cap upstream is
/// reported by `upgrade` and installed anyway (WF-7, WF-32). The cap is the one
/// WF-30 reason that is a property of the file rather than of its `meta`, and
/// the only one that can fire while `meta` reads perfectly.
// spec: WF-7 WF-30 WF-31 WF-32
#[test]
fn wf32_upgrade_reports_a_workflow_that_grew_past_the_cap_and_installs_it() {
    let sb = Sandbox::new();
    sb.write_workflow("big", "big");
    sb.commit_src("add workflow");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    let learn = sb.mind(&["learn", "workflow:big"]);
    assert!(learn.success, "learn: {}", learn.stderr);
    assert!(
        !learn.stderr.contains("byte cap"),
        "the under-cap install must draw no overage warning: {}",
        learn.stderr
    );

    // Same readable `meta`, padded past the cap by a trailing comment.
    let mut body = String::from("export const meta = {\n  name: 'big',\n  description: 'Big'\n}\n");
    body.push_str("// ");
    body.push_str(&"p".repeat(524_288));
    body.push('\n');
    sb.write_workflow_raw("big", &body);
    sb.commit_src("pad past the cap");
    assert!(sb.mind(&["sync"]).success);

    let up = sb.mind(&["upgrade", "--yes"]);
    // spec: WF-32 -- reported, never enforced.
    assert!(
        up.success,
        "an over-cap upgrade must still succeed: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    assert!(
        up.stderr
            .contains("over the harness's 524288-byte cap; installed anyway"),
        "upgrade must report the overage: {}",
        up.stderr
    );
    assert!(
        up.stderr.contains("workflow:big"),
        "the overage must be keyed to the item: {}",
        up.stderr
    );
    let installed = std::fs::metadata(sb.claude_home.join("workflows/big.js"))
        .expect("the over-cap workflow must still be installed");
    assert!(
        installed.len() > 524_288,
        "the over-cap file must be what landed, unmodified: {} bytes",
        installed.len()
    );
}

// ---- the early return: only what was upgraded is reported -----------------

/// Upgrading a NON-workflow item while a defective workflow sits installed
/// reports nothing about the workflow. `warn_workflows` returns early when
/// nothing in the batch is a workflow, and the guarantee that matters is not
/// the saved scan but this silence: the comparison set is the whole installed
/// set, so a loop keyed off that set instead of off the upgraded keys would
/// re-report every old defect on every unrelated upgrade.
// spec: WF-24 WF-29
#[test]
fn an_upgrade_that_touches_no_workflow_reports_no_workflow_warning() {
    let sb = Sandbox::new();
    // An installed workflow whose harness name already diverges: every run
    // that DOES look at it warns, so silence here is meaningful.
    sb.write_workflow("alpha", "not-alpha");
    write_file(
        &sb.src.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );
    sb.commit_src("add a workflow and a skill");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    let learn_w = sb.mind(&["learn", "workflow:alpha"]);
    assert!(learn_w.success, "learn workflow: {}", learn_w.stderr);
    assert!(
        learn_w
            .stderr
            .contains("the harness resolves it as 'not-alpha'"),
        "the fixture's divergence must be real, or this test proves nothing: {}",
        learn_w.stderr
    );
    assert!(sb.mind(&["learn", "skill:review"]).success);

    // Only the skill changes upstream.
    write_file(
        &sb.src.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff\n---\n# review\n\nNow with detail.\n",
    );
    sb.commit_src("edit the skill");
    assert!(sb.mind(&["sync"]).success);

    let up = sb.mind(&["upgrade", "skill:review", "--yes"]);
    assert!(
        up.success,
        "upgrade must succeed: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    assert!(
        up.stdout.contains("upgraded"),
        "the skill must actually have upgraded: {}",
        up.stdout
    );
    assert!(
        !up.stderr.contains("the harness resolves it as"),
        "an upgrade that touched no workflow must say nothing about one: {}",
        up.stderr
    );
    assert!(
        !up.stderr.contains("will not load this workflow"),
        "an upgrade that touched no workflow must say nothing about one: {}",
        up.stderr
    );
}

// ---- the failure path: `upgrade` warns like `learn` -----------------------

/// A workflow that upgraded before a later item in the same batch failed is
/// still warned about. `learn` has always done this (its `warn_workflows` call
/// precedes the `match failure`), and `upgrade`'s early `return Err(e)` paths
/// now do the same: the item is live on disk and recorded, and the next
/// `upgrade` finds it current and never looks at it again, so a warning skipped
/// here is a defect no run ever reports.
///
/// The upgrade batch is ordered by manifest key (a `BTreeMap` of `kind:name`),
/// so `workflow:alpha` is applied before `workflow:zbad` fails on an
/// unresolvable `{{ns:}}` reference. The assertions below pin: the batch fails,
/// alpha's new content IS live (so the warning is about a real, applied
/// upgrade), and the WF-24 divergence it introduced is reported.
// spec: WF-24 LIFE-48
#[test]
fn upgrade_warns_about_a_workflow_applied_before_a_later_item_failed() {
    let sb = Sandbox::new();
    sb.write_workflow("alpha", "alpha");
    sb.write_workflow("zbad", "zbad");
    sb.commit_src("add workflows");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    assert!(sb.mind(&["learn", "workflow:alpha"]).success);
    assert!(sb.mind(&["learn", "workflow:zbad"]).success);

    // alpha's harness name diverges; zbad gains a reference to no sibling, so
    // its install fails at expansion time, after alpha has been applied.
    sb.write_workflow("alpha", "renamed-upstream");
    sb.write_workflow_raw(
        "zbad",
        "export const meta = {\n  name: 'zbad',\n  description: 'Bad'\n}\n\
         // {{ns:no-such-sibling}}\n",
    );
    sb.commit_src("diverge alpha, break zbad");
    assert!(sb.mind(&["sync"]).success);

    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        !up.success,
        "the batch must fail on zbad's bad reference: stdout={} stderr={}",
        up.stdout, up.stderr
    );

    // spec: LIFE-48 -- alpha was applied and persisted before the failure.
    let installed = std::fs::read_to_string(sb.claude_home.join("workflows/alpha.js"))
        .expect("alpha must still be installed");
    assert!(
        installed.contains("name: 'renamed-upstream'"),
        "alpha's upgrade must have been applied before zbad failed: {installed}"
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.success && recall.stdout.contains("alpha"),
        "alpha must remain recorded after the failed batch: {} {}",
        recall.stdout,
        recall.stderr
    );

    // The fix: the applied item's new divergence is reported by the run that
    // applied it, batch failure or not.
    assert!(
        up.stderr
            .contains("the harness resolves it as 'renamed-upstream'"),
        "the workflow applied before the batch failed must still be warned \
         about -- no later run will look at it again: {}",
        up.stderr
    );
}

/// The rename path's failure tail: `install_item` succeeded, so the new copy is
/// live under the new name, and only the removal of the OLD item failed. The
/// item that just landed is the one whose divergence nothing else will report,
/// since the manifest now records it as current.
///
/// The other two failure tails (`link_reconciled`, and the in-place branch's
/// removal of a link the new install no longer owns) carry the same call, but
/// no workflow can reach either of them with one in `applied_keys`: the batch
/// runs in manifest-key order and `workflow` sorts last of the six kinds, so
/// nothing is applied after a workflow except another workflow, and neither
/// tail is reachable for a workflow itself (an in-place workflow upgrade cannot
/// change its own link set, which is `workflows/<name>.js` either way). Their
/// calls are defensive, and this test covers the one that bites.
// spec: WF-24 LIFE-48
#[test]
fn upgrade_warns_when_the_old_item_fails_to_uninstall_after_a_rename() {
    let sb = Sandbox::new();
    sb.write_workflow("deploy", "deploy");
    write_file(
        &sb.src.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"deploy\"\n\
         path = \"workflows/deploy.js\"\nuninstall = \"exit 3\"\n",
    );
    sb.commit_src("declare the workflow with a failing uninstall hook");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    assert!(sb.mind(&["learn", "workflow:deploy"]).success);

    // A new upstream prefix renames the item (workflow:deploy ->
    // workflow:jk:deploy), which is the branch that uninstalls the old copy.
    // `meta.name` stays the bare 'deploy', so the rename is what introduces the
    // divergence: exactly the defect no later run would look for.
    write_file(
        &sb.src.join("mind.toml"),
        "[source]\nprefix = \"jk\"\n\n\
         [[items]]\nkind = \"workflow\"\nname = \"deploy\"\n\
         path = \"workflows/deploy.js\"\nuninstall = \"exit 3\"\n",
    );
    sb.commit_src("add a prefix");
    assert!(sb.mind(&["sync"]).success);

    // The hook only runs where it can be consented to: in a non-interactive
    // context it is skipped with a note, and the rename succeeds.
    let up = sb.mind_interactive(&["upgrade", "--yes"], "y\n");
    assert!(
        !up.success,
        "the failing uninstall hook must fail the run: stdout={} stderr={}",
        up.stdout, up.stderr
    );
    assert!(
        sb.claude_home.join("workflows/jk:deploy.js").exists(),
        "the renamed copy is live on disk before the failure: stdout={} stderr={}",
        up.stdout,
        up.stderr
    );
    assert!(
        up.stderr.contains("the harness resolves it as 'deploy'"),
        "the divergence the rename introduced must be reported by the run that \
         introduced it: {}",
        up.stderr
    );
}

// ---- WF-55: an over-cap workflow does not take the source's scan with it ----

/// A workflow past mind's own metadata cap (DSC-91, 8 MiB) is catalogued with
/// no readable metadata rather than failing the scan, so every OTHER item of
/// that source stays reachable, and the oversized one installs like any other
/// unloadable workflow (WF-31) with the WF-30 warning.
///
/// The `learn workflow:fine` step is the regression: before WF-55 the capped
/// read ran for every item of the source in one pass, so the oversized sibling
/// made this (and every other catalog-scanning verb) fail outright.
// spec: WF-55 WF-56 WF-31
#[test]
fn an_over_cap_workflow_installs_and_leaves_its_source_scannable() {
    let sb = Sandbox::new();
    sb.write_workflow("fine", "fine");
    // Valid UTF-8, a complete `meta` at the top, and padded past mind's 8 MiB
    // read cap: mind cannot see the `meta` it does have, which is the point.
    let mut huge = workflow_js("huge", "huge");
    huge.push_str("// ");
    huge.push_str(&"p".repeat(8 * 1024 * 1024));
    huge.push('\n');
    sb.write_workflow_raw("huge", &huge);
    sb.commit_src("add workflows");

    let spec = sb.src_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);

    let fine = sb.mind(&["learn", "workflow:fine"]);
    assert!(
        fine.success,
        "the healthy sibling must still be installable: stdout={} stderr={}",
        fine.stdout, fine.stderr
    );
    assert!(
        !fine.stderr.contains("8 MiB size cap"),
        "scanning the source must not fail on the oversized sibling: {}",
        fine.stderr
    );

    let huge_run = sb.mind(&["learn", "workflow:huge"]);
    assert!(
        huge_run.success,
        "an unloadable workflow installs anyway (WF-31): stdout={} stderr={}",
        huge_run.stdout, huge_run.stderr
    );
    assert!(
        sb.claude_home.join("workflows/huge.js").exists(),
        "the oversized workflow must be linked into the lobe"
    );
    // spec: WF-56 -- mind's own cap is reported as mind's own, naming the cap
    // and the flag that raises it, not as a verdict about the harness mind
    // never read enough of the file to reach.
    assert!(
        huge_run
            .stderr
            .contains("over mind's own 8 MiB metadata read cap"),
        "mind's own cap is warned about as mind's own, not as an error: {}",
        huge_run.stderr
    );
    assert!(
        huge_run.stderr.contains("--max-metadata-size"),
        "and names the flag that raises it: {}",
        huge_run.stderr
    );
    assert!(
        !huge_run
            .stderr
            .contains("the harness will not load this workflow: mind read no"),
        "a file mind never read is not a file mind can call unloadable: {}",
        huge_run.stderr
    );
    assert!(
        !huge_run.stderr.contains("8 MiB size cap"),
        "the cap bounds mind's read; it is not reported as a failure: {}",
        huge_run.stderr
    );
}
