//! Integration tests for managed-policy allowlist matching against extended
//! source identities: item-link (`#path`) and consumer-alias (`@alias`)
//! instances (spec/policy.md POL-67).
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture: a local git repo addressed by filesystem path (and, for the
//! item-link cases, the `file://` deep-link form), with MIND_HOME/CLAUDE_HOME
//! pointed at a temp dir and the managed policy injected via
//! `MIND_POLICY_FILE` (honored only when no real system policy file exists,
//! POL-2). Follows the `MIND_POLICY_FILE` pattern used in tests/cli.rs and
//! tests/cli_item_link.rs.

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
    /// A source repo named `agents` with two convention skills (`review`,
    /// `extra`), suitable for both a plain meld and an item-link meld.
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
        let base = std::env::temp_dir().join(format!("mind-pol-{}-{n}", std::process::id()));
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

    /// The plain repo path spec (for an ordinary, non-link meld).
    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// A `file://` item link into this sandbox's source repo (LNK-1).
    fn link(&self, tail: &str) -> String {
        format!("file://{}/{tail}", self.source.to_string_lossy())
    }

    /// Write a policy TOML to the sandbox base and return its absolute path
    /// string, for use as the `MIND_POLICY_FILE` env value.
    fn write_policy(&self, body: &str) -> String {
        let path = self.base.join("policy.toml");
        write(&path, body);
        path.to_string_lossy().into_owned()
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

const NOT_PERMITTED: &str = "source not permitted by the managed policy's allowlist";

// The `MindError::SourceNotAllowed` Display text (src/error.rs), distinct from
// the `NOT_PERMITTED` skip-line wording used by the sync/upgrade/learn loops
// above: `"source '{identity}' is not permitted by the managed policy's
// allowlist"`. Both share this substring.
const SOURCE_NOT_ALLOWED: &str = "is not permitted by the managed policy's allowlist";

// --- POL-67: allowlist matching uses the base repo identity everywhere -----
//
// The base repo identity for these fixtures is `local/<base>/agents` (the
// sandbox's temp-dir name as owner, `agents` as repo). The allow pattern
// `local/*/agents` names only the base identity: no `#path`/`@alias` suffix.
// Before the POL-67 fix, `sync`/`upgrade` matched the FULL extended identity
// against this pattern and refused (a link/alias instance was admitted at
// `meld`/`learn` time via `base_identity()`, per LNK-11, but then skipped at
// every later lifecycle step). After the fix, `Policy::allow_matches` itself
// truncates to the base identity, so every call site is consistent.

#[test]
fn sync_and_upgrade_admit_item_link_instance_under_locked_allowlist() {
    // spec: POL-67
    // An item-link instance's identity is `host/owner/repo#path` (LNK-4). The
    // allow pattern names only the base repo, so a locked policy that admits
    // the repo must keep admitting the link instance at sync and upgrade, not
    // just at the initial `learn`.
    let sb = Sandbox::new();
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");

    let learn = sb.mind_env(
        &["learn", &sb.link("tree/main/skills/review")],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        learn.success,
        "learn of an item link into an allowed repo must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let sync = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        sync.success,
        "sync must succeed for an item-link instance of an allowed repo: {} {}",
        sync.stdout, sync.stderr
    );
    assert!(
        !sync.stdout.contains(NOT_PERMITTED),
        "sync must not skip the item-link instance as not permitted: {}",
        sync.stdout
    );

    let upgrade = sb.mind_env(
        &["upgrade", "--no-sync"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        upgrade.success,
        "upgrade must succeed for an item-link instance of an allowed repo: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stdout.contains(NOT_PERMITTED),
        "upgrade must not skip the item-link instance as not permitted: {}",
        upgrade.stdout
    );
}

#[test]
fn sync_and_upgrade_admit_alias_instance_under_locked_allowlist() {
    // spec: POL-67
    // An `@alias`-forked instance's identity is `host/owner/repo@alias`. The
    // allow pattern names only the base repo, so a locked policy that admits
    // the repo must keep admitting the aliased instance at sync and upgrade.
    let sb = Sandbox::new();
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &["meld", &spec, "--as", "myalias", "--register-only"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        meld.success,
        "melding an aliased instance of an allowed repo must succeed: {} {}",
        meld.stdout, meld.stderr
    );
    let learn = sb.mind_env(
        &["learn", "myalias:review"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        learn.success,
        "learn of the aliased instance's item must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let sync = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        sync.success,
        "sync must succeed for an aliased instance of an allowed repo: {} {}",
        sync.stdout, sync.stderr
    );
    assert!(
        !sync.stdout.contains(NOT_PERMITTED),
        "sync must not skip the aliased instance as not permitted: {}",
        sync.stdout
    );

    let upgrade = sb.mind_env(
        &["upgrade", "--no-sync"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        upgrade.success,
        "upgrade must succeed for an aliased instance of an allowed repo: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stdout.contains(NOT_PERMITTED),
        "upgrade must not skip the aliased instance as not permitted: {}",
        upgrade.stdout
    );
}

#[test]
fn sync_and_upgrade_admit_composed_link_and_alias_instance_under_locked_allowlist() {
    // spec: POL-67
    // Both suffixes composed: `host/owner/repo#path@alias` (the alias always
    // follows the path, per Source::compute_name). The allow pattern still
    // names only the base repo.
    let sb = Sandbox::new();
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");

    let meld = sb.mind_env(
        &[
            "meld",
            &sb.link("tree/main/skills/review"),
            "--namespace",
            "myalias",
            "--register-only",
        ],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        meld.success,
        "melding a namespaced item-link instance of an allowed repo must succeed: {} {}",
        meld.stdout, meld.stderr
    );
    let learn = sb.mind_env(
        &["learn", "myalias:review"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        learn.success,
        "learn of the namespaced link instance's item must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let sync = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        sync.success,
        "sync must succeed for a composed link+alias instance of an allowed repo: {} {}",
        sync.stdout, sync.stderr
    );
    assert!(
        !sync.stdout.contains(NOT_PERMITTED),
        "sync must not skip the composed instance as not permitted: {}",
        sync.stdout
    );

    let upgrade = sb.mind_env(
        &["upgrade", "--no-sync"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        upgrade.success,
        "upgrade must succeed for a composed link+alias instance of an allowed repo: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stdout.contains(NOT_PERMITTED),
        "upgrade must not skip the composed instance as not permitted: {}",
        upgrade.stdout
    );
}

// --- POL-67: negative path -- truncation must not widen the allowlist ------

#[test]
fn meld_still_refuses_alias_instance_of_a_different_disallowed_repo_under_lock() {
    // spec: POL-67
    // The allow pattern matches a repo named `agents`; this fixture's repo is
    // named `other-repo`, so its base identity never matches regardless of the
    // `@alias` suffix. Before matching, `allow_matches` truncates
    // `local/<owner>/other-repo@myalias` down to `local/<owner>/other-repo` --
    // truncation strips the suffix marker, it does not turn the identity into
    // something a sibling pattern would match. Confirms truncation cannot
    // accidentally widen the allowlist to admit an aliased instance of a
    // genuinely disallowed repo.
    let sb = Sandbox::bare("other-repo");
    sb.write_and_commit(
        "skills/x/SKILL.md",
        "---\ndescription: x skill\n---\n# x skill\n",
    );
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &["meld", &spec, "--namespace", "myalias", "--register-only"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !meld.success,
        "melding an aliased instance of a disallowed repo must still be refused: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains(SOURCE_NOT_ALLOWED),
        "expected a SourceNotAllowed refusal in stderr: {}",
        meld.stderr
    );
}

// --- POL-67: negative path end-to-end -- meld-time refusal ------------------

#[test]
fn meld_refuses_alias_instance_of_a_repo_outside_the_allowlist() {
    // spec: POL-67
    // Item 3: melding an ALIASED instance of a repo that is not in the
    // allowlist at all (not merely a sibling of an allowed pattern) is
    // refused at meld time with SourceNotAllowed, confirming the truncation
    // change did not accidentally admit it.
    let sb = Sandbox::new();
    let policy = sb.write_policy("[sources]\nlock = true\nallow = []\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &["meld", &spec, "--namespace", "myalias", "--register-only"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !meld.success,
        "melding an aliased instance of a repo outside the allowlist must be refused: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains(SOURCE_NOT_ALLOWED),
        "expected a SourceNotAllowed refusal in stderr: {}",
        meld.stderr
    );
}

#[test]
fn learn_refuses_item_link_instance_of_a_repo_outside_the_allowlist() {
    // spec: POL-67
    // Item 3: `learn <link-url>` registers the link instance via the same
    // `meld()` gate as a plain meld (LNK-6). A repo outside the allowlist
    // must be refused end-to-end, not just for a plain (non-link) meld.
    let sb = Sandbox::new();
    let policy = sb.write_policy("[sources]\nlock = true\nallow = []\n");

    let learn = sb.mind_env(
        &["learn", &sb.link("tree/main/skills/review")],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !learn.success,
        "learn of an item link into a repo outside the allowlist must be refused: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        learn.stderr.contains(SOURCE_NOT_ALLOWED),
        "expected a SourceNotAllowed refusal in stderr: {}",
        learn.stderr
    );
}

// --- POL-68: the base identity is structural, not a string scan ------------
//
// `#` and `@` are suffix markers only by convention: `owner` and `repo` may
// legitimately contain them (CLI-204 rejects them only in `host`). Deriving the
// base identity by scanning for the first marker therefore both admits a repo
// it must not and refuses one it must. Both directions are driven end-to-end
// here through the real binary, since the whole point is that the string form
// and the structural form disagree.

#[test]
fn meld_refuses_a_repo_whose_name_merely_starts_with_an_allowed_name() {
    // spec: POL-68
    // spec: STO-64
    // A directory named `blessed@evil` is a different repo, not the allowed
    // `blessed` with a consumer alias, and must never be admitted by an
    // allowlist naming `blessed`. Two independent layers now stop it, and this
    // asserts the outer one end to end: STO-64 refuses a repo name carrying the
    // alias marker at parse time, before the allowlist is consulted at all. The
    // inner layer (POL-68: matching never truncates an identity down to a
    // shorter allowed one) is asserted directly in src/policy.rs, since it is
    // no longer reachable through a real meld.
    let sb = Sandbox::bare("blessed@evil");
    sb.write_and_commit(
        "skills/x/SKILL.md",
        "---\ndescription: x skill\n---\n# x skill\n",
    );
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/blessed\"]\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &["meld", &spec, "--register-only"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !meld.success,
        "a repo named `blessed@evil` must not be admitted by an allowlist naming `blessed`: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains("is not a usable repo spec") && meld.stderr.contains("repo"),
        "expected the parse-time refusal naming the repo part: {}",
        meld.stderr
    );
}

#[test]
fn meld_admits_a_repo_whose_owner_segment_contains_a_marker() {
    // spec: POL-68
    // The other direction: an owner directory legitimately named `proj@v2`
    // matches a wildcard over that segment. A first-marker string scan
    // truncates `local/proj@v2/agents` to the two-segment `local/proj`, which
    // matches no three-segment pattern, so the meld is refused with a message
    // naming an identity the policy does allow.
    let sb = Sandbox::bare("proj@v2/agents");
    sb.write_and_commit(
        "skills/x/SKILL.md",
        "---\ndescription: x skill\n---\n# x skill\n",
    );
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &["meld", &spec, "--register-only"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        meld.success,
        "an owner segment containing `@` must still match a wildcard over that segment: {} {}",
        meld.stdout, meld.stderr
    );

    // And it stays admitted at the later lifecycle gates, not just at meld.
    let sync = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !sync.stdout.contains(NOT_PERMITTED) && !sync.stderr.contains(NOT_PERMITTED),
        "sync must not skip an admitted source: {} {}",
        sync.stdout,
        sync.stderr
    );
}

// --- CLI-204: unsafe identity parts are refused before any clone ------------

#[test]
fn meld_refuses_a_traversing_ssh_host_before_touching_the_filesystem() {
    // spec: CLI-204
    // The ssh form splits only on the first `:`, so `host` absorbs whatever
    // precedes it. A traversing host would be joined into the clone path, which
    // `meld` deletes with `remove_dir_all` before cloning, and would forge extra
    // identity segments for policy matching. Refused at parse time, so nothing
    // on disk is touched.
    // The clone path is `<mind_home>/sources/<host>/<owner>/<repo>`, so a host
    // of `../../canary` lands the whole path on `<base>/canary/owner/repo`.
    // That directory is populated here, so an unvalidated host would delete it
    // outright: `meld` calls `remove_dir_all` on an existing clone dir before
    // cloning.
    let sb = Sandbox::new();
    let victim = sb.base.join("canary").join("owner").join("repo");
    write(&victim.join("keep.txt"), "do not delete\n");

    let meld = sb.mind_env(&["meld", "git@../../canary:owner/repo"], &[]);
    assert!(
        victim.join("keep.txt").exists(),
        "the traversed directory outside the sources tree was deleted: {} {}",
        meld.stdout,
        meld.stderr
    );
    assert!(
        !meld.success,
        "a traversing ssh host must be refused: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains("is not a usable repo spec") && meld.stderr.contains("host"),
        "expected an UnsafeRepoSpec refusal naming the host part: {}",
        meld.stderr
    );
}

// --- POL-67: the install-hook gate site (HOOK-11's rerun_source_hooks) -----
//
// `rerun_source_hooks` (src/commands.rs) gates on
// `policy.allow_matches(&source.name)`, and `source.name` is the FULL extended
// identity (`Source::compute_name`), unlike the initial `meld` gate, which
// checks `source.base_identity()` directly (LNK-11) and so needed no fix. This
// is the call site most exposed to the bug this fix closes: before POL-67, an
// aliased instance's hook would be silently skipped as "not permitted" on every
// `upgrade` after the first, even though the repo itself is allowed.

#[test]
fn upgrade_reruns_install_hook_for_admitted_alias_instance_under_locked_allowlist() {
    // spec: POL-67
    // An `@alias`-suffixed instance's install hook must still be offered/run on
    // `upgrade` once its source has advanced, under a locked policy whose
    // `allow` pattern names only the base repo. Before the fix,
    // `policy.allow_matches("local/<owner>/agents@myalias")` matched against
    // `local/*/agents` would fail (the untruncated identity's last segment is
    // `agents@myalias`, not `agents`), so the hook rerun would be wrongly
    // skipped as policy-blocked.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\ninstall = \"echo RAN >> counter.log\"\n",
    );
    let policy = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");
    let spec = sb.source_spec();

    let meld = sb.mind_env(
        &[
            "meld",
            &spec,
            "--namespace",
            "myalias",
            "--register-only",
            "--dangerously-skip-install-hook-check",
        ],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        meld.success,
        "melding an aliased instance of an allowed repo must succeed: {} {}",
        meld.stdout, meld.stderr
    );

    // The source is local and unpinned, so it is "linked" (CLI-27): the hook
    // runs directly in sb.source, regardless of the alias.
    let counter = sb.source.join("counter.log");
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap_or_default(),
        "RAN\n",
        "the install hook must have run once at meld time"
    );

    // Advance the source so upgrade's hook-rerun pass has a pending hook.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
    );

    let sync = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        sync.success,
        "sync must succeed for the aliased instance: {} {}",
        sync.stdout, sync.stderr
    );

    let upgrade = sb.mind_env(
        &[
            "upgrade",
            "--no-sync",
            "-y",
            "--dangerously-skip-install-hook-check",
        ],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        upgrade.success,
        "upgrade must succeed for the aliased instance: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stdout.contains(NOT_PERMITTED),
        "upgrade must not report the aliased instance's install hook as policy-blocked: {}",
        upgrade.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap_or_default(),
        "RAN\nRAN\n",
        "the install hook must have re-run on upgrade, proving the hook-gate site \
         admitted the aliased instance rather than silently skipping it"
    );
}

// --- CLI-209: a re-meld's `--pin` re-pins an already-registered source, and
// the meld-time policy gates (POL-11 allowlist, POL-20 require-pinned) apply
// to that re-pin exactly as they do at a first meld. ------------------------

#[test]
fn remeld_pin_honors_require_pinned_policy() {
    // spec: CLI-209, POL-20 - a re-meld's `--pin branch=<name>` would persist a
    // floating follow-branch pin, which `[sources].pinned = true` forbids
    // (POL-20); the re-pin path must refuse it exactly like a first meld does,
    // not silently apply it because the source is already registered.
    let sb = Sandbox::new();
    git(&sb.source, &["tag", "v1.0"]);
    git(&sb.source, &["branch", "stable"]);
    let spec = sb.source_spec();
    let policy =
        sb.write_policy("[sources]\npinned = true\nlock = true\nallow = [\"local/*/agents\"]\n");

    // First meld satisfies require-pinned via a tag freeze.
    let first = sb.mind_env(
        &["meld", &spec, "--register-only", "--pin", "v1.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        first.success,
        "first meld: {} {}",
        first.stdout, first.stderr
    );

    let r = sb.mind_env(
        &["meld", &spec, "--register-only", "--pin", "branch=stable"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "a re-pin to a floating branch must be refused under require-pinned: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("must be pinned"),
        "expected the require-pinned refusal, got: {}",
        r.stderr
    );
}

#[test]
fn remeld_pin_refused_when_source_falls_outside_locked_allowlist() {
    // spec: CLI-209, POL-11 - the allowlist gate applies to a re-pin exactly
    // as it does at a first meld: a source whose identity is no longer in a
    // (now-locked, or since-narrowed) allowlist must not be silently re-pinned
    // around the gate.
    let sb = Sandbox::new();
    git(&sb.source, &["tag", "v1.0"]);
    let spec = sb.source_spec();

    let allowed = sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/agents\"]\n");
    let first = sb.mind_env(
        &["meld", &spec, "--register-only"],
        &[("MIND_POLICY_FILE", allowed.as_str())],
    );
    assert!(
        first.success,
        "first meld: {} {}",
        first.stdout, first.stderr
    );

    // The allowlist is narrowed to no longer admit this source.
    let narrowed =
        sb.write_policy("[sources]\nlock = true\nallow = [\"local/*/some-other-repo\"]\n");
    let r = sb.mind_env(
        &["meld", &spec, "--register-only", "--pin", "v1.0"],
        &[("MIND_POLICY_FILE", narrowed.as_str())],
    );
    assert!(
        !r.success,
        "a re-pin of a source dropped from the locked allowlist must be refused: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains(SOURCE_NOT_ALLOWED),
        "expected the SourceNotAllowed refusal, got: {}",
        r.stderr
    );
}

// --- POL-60 x CLI-213: the headless install pass scans ONE source -----------

#[test]
fn auto_meld_install_records_a_source_it_cannot_scan_at_all() {
    // spec: POL-60 CLI-213
    // `install_provisioned_items` scans exactly the one source it was asked to
    // provision. Routed through the whole-registry `catalog::scan`, it inherited
    // the CLI-213 degradation: a `LinkedSourceGone` source scanned as
    // `Ok(vec![])`, the function took the `source_items.is_empty()` early return,
    // and it reported NO failures -- so POL-60's warn-record-continue-nonzero
    // accounting recorded nothing and the run looked like "this source had
    // nothing left to install" rather than "this source could not be read".
    // Melded, installed, then the working tree vanishes; the next `sync` must
    // name the source in the auto_meld install accounting.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let policy = sb.write_policy(&format!(
        "[[sources.auto_meld]]\nrepo = \"{}\"\ninstall = true\n",
        spec.replace('\\', "\\\\")
    ));

    let first = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        first.success,
        "auto_meld provisioning must succeed: {} {}",
        first.stdout, first.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the provisioned source's items must be installed by the first sync: {} {}",
        first.stdout,
        first.stderr
    );

    // The linked working tree goes away (CLI-212's exact scenario).
    std::fs::remove_dir_all(&sb.source).expect("remove the linked working tree");

    let second = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        second.stderr.contains("auto_meld item install failed"),
        "the install pass must record the unscannable source as a failure, not \
         return an empty item list: {} {}",
        second.stdout,
        second.stderr
    );
    assert!(
        second.stderr.contains("linked working tree") && second.stderr.contains("is gone"),
        "the recorded failure must carry the LinkedSourceGone cause: {} {}",
        second.stdout,
        second.stderr
    );
    assert!(
        !second.success,
        "POL-60 records the failure and exits non-zero: {} {}",
        second.stdout, second.stderr
    );
}

#[test]
fn auto_meld_install_records_one_bad_item_and_still_installs_the_rest() {
    // spec: POL-60
    // The other half of the accounting the CLI-213 change moved onto: a
    // WHOLE-SOURCE scan failure short-circuits `install_provisioned_items`
    // before the per-item loop, so the test above never enters that loop. This
    // one does. POL-60 promises warn, record, CONTINUE, non-zero exit, and
    // "continue" is the part with no coverage: an early `return` on the first
    // failing item would satisfy every assertion about the failure itself while
    // silently dropping every later item.
    //
    // One item carries a `{{ns:}}` reference to a sibling that does not exist,
    // which fails at install time (BadReference) with the source itself
    // perfectly readable. The other item must still land.
    let sb = Sandbox::new();
    // `aaa-broken` sorts before `review`/`extra` so, whatever order the catalog
    // yields, the failure is not guaranteed to be last: an early return would
    // be observable.
    sb.write_and_commit(
        "skills/aaa-broken/SKILL.md",
        "---\ndescription: references a sibling that does not exist\n---\n\
         # broken\nSee {{ns:no-such-sibling}} for details.\n",
    );
    let spec = sb.source_spec();
    let policy = sb.write_policy(&format!(
        "[[sources.auto_meld]]\nrepo = \"{}\"\ninstall = true\n",
        spec.replace('\\', "\\\\")
    ));

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.stderr.contains("auto_meld item install failed"),
        "the failing item must be recorded through the POL-60 accounting: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        r.stderr.contains("skill:aaa-broken"),
        "the recorded failure must name the ITEM, not just the source: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !r.success,
        "POL-60 exits non-zero when any item failed: {} {}",
        r.stdout, r.stderr
    );

    // CONTINUE: the sibling items are installed anyway.
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "a failing item must not abort the rest of the source's install: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        sb.claude_home.join("skills/extra").exists(),
        "every other item must still install: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !sb.claude_home.join("skills/aaa-broken").exists(),
        "the failing item must not be left half-installed: {} {}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn auto_meld_install_under_json_emits_one_document() {
    // CLI-217: "under --json, stdout carries exactly one JSON document and
    // nothing else". POL-58's headless install pass calls
    // `install_provisioned_items`, which called `learn` once per item, and
    // `learn` emits its OWN CLI-153 result object under `--json`. `sync` then
    // printed its own. So the machine-driven path -- a managed fleet running
    // `mind --json sync` on a schedule, which is precisely who `auto_meld`
    // exists for -- got N+1 JSON documents concatenated on stdout.
    //
    // Every OTHER line on this path was already correctly guarded (`if !out.json
    // { eprintln!(...) }` for the POL-60 accounting), which is what made the
    // leak easy to miss: it is not prose, it is well-formed JSON in the wrong
    // quantity.
    //
    // The provisioning pass now installs through the silent `learn_collecting`
    // under `--json` and hands its keys back to `sync`, which reports them in
    // its one object.
    // spec: CLI-217 POL-58 POL-60
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let policy = sb.write_policy(&format!(
        "[[sources.auto_meld]]\nrepo = \"{}\"\ninstall = true\n",
        spec.replace('\\', "\\\\")
    ));

    let r = sb.mind_env(
        &["--json", "sync"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(r.success, "sync --json: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert_eq!(
        doc["action"], "sync",
        "the one document is the invoked verb's, not a provisioned item's: {doc:#}"
    );
    // The provisioned items are still ACCOUNTED FOR, in sync's object. Without
    // this the leak could be "fixed" by silently dropping what the pass did.
    assert!(
        doc["installed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "skill:review")),
        "sync must report what the auto_meld install pass installed: {doc:#}"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the provisioned item must actually be installed: {doc:#}"
    );
}

#[test]
fn auto_meld_install_in_text_mode_still_narrates_each_item() {
    // The other side of the CLI-217 fix above: the silent install path is
    // reserved for `--json`. An interactive `mind sync` must still print the
    // per-item "learned ..." lines, or the fix would have traded a machine-
    // readability bug for a human-visibility one -- and every assertion in the
    // test above would still pass.
    // spec: POL-58
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let policy = sb.write_policy(&format!(
        "[[sources.auto_meld]]\nrepo = \"{}\"\ninstall = true\n",
        spec.replace('\\', "\\\\")
    ));

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("learned skill:review"),
        "text-mode sync must narrate the provisioned install: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn auto_meld_install_under_json_records_one_bad_item_and_still_installs_the_rest() {
    // The JSON twin of `auto_meld_install_records_one_bad_item_and_still_installs_the_rest`
    // above: `install_provisioned_items` branches on `json_mode()` to call
    // `learn_collecting` instead of `learn` (CLI-217), which is different code
    // from the text-mode path that test drives. POL-60's warn/record/continue/
    // non-zero-exit accounting is asserted there but never through the JSON
    // branch: an early `return` on the first failing item inside the
    // `learn_collecting` arm, or a swallowed failure that stopped recording it
    // in `provision_failures`, would satisfy every text-mode assertion while
    // breaking the JSON path silently.
    // spec: CLI-217 POL-58 POL-60
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/aaa-broken/SKILL.md",
        "---\ndescription: references a sibling that does not exist\n---\n\
         # broken\nSee {{ns:no-such-sibling}} for details.\n",
    );
    let spec = sb.source_spec();
    let policy = sb.write_policy(&format!(
        "[[sources.auto_meld]]\nrepo = \"{}\"\ninstall = true\n",
        spec.replace('\\', "\\\\")
    ));

    let r = sb.mind_env(
        &["--json", "sync"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "POL-60 exits non-zero when any item failed, even under --json: {} {}",
        r.stdout, r.stderr
    );
    // stdout must still be exactly one JSON document: CLI-181's error envelope,
    // since a run reporting a POL-60 failure exits non-zero.
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document even on a \
             POL-60 failure ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        doc.get("error").is_some(),
        "a non-zero exit must answer with the CLI-181 error envelope: {doc:#}"
    );
    // Pre-existing (not part of this round's diff): the per-item
    // "auto_meld item install failed" detail line is itself guarded by
    // `if !out.json { eprintln!(...) }`, so under --json it is dropped
    // entirely rather than routed to stderr the way CLI-217 routes every
    // other note/warn. The failure is still counted (the envelope's message
    // says "1 of 2 source(s)"), but the human-readable detail the message
    // text ("see the messages above") implies is not actually there under
    // --json. Documented, not asserted as correct: a JSON caller should not
    // rely on stderr detail here.
    assert!(
        !r.stderr.contains("auto_meld item install failed"),
        "if this line starts appearing on stderr under --json, update this \
         test to assert it (that would be a strict improvement over today's \
         behavior, not a regression): {} {}",
        r.stdout,
        r.stderr
    );

    // CONTINUE: the sibling items are installed anyway, exactly as in the
    // text-mode case -- the JSON branch must not stop early on the first
    // failure.
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "a failing item must not abort the rest of the source's install under \
         --json: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        sb.claude_home.join("skills/extra").exists(),
        "every other item must still install under --json: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !sb.claude_home.join("skills/aaa-broken").exists(),
        "the failing item must not be left half-installed: {} {}",
        r.stdout,
        r.stderr
    );
}
