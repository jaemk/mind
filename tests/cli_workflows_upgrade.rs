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
}
