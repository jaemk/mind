//! Integration tests for release version pins (spec/versions.md).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo whose commits carry semver release tags, melded by filesystem path
//! with `MIND_HOME`/`CLAUDE_HOME` pointed at temp dirs. No network.

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
    success: bool,
}

impl Sandbox {
    /// An empty git repo named `name` on branch `main`.
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-versions-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        std::fs::create_dir_all(&source).unwrap();
        for args in [
            vec!["-c", "init.defaultBranch=main", "init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(&source, &args);
        }
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
    }

    /// Commit an agent whose description is `desc`, plus `mind_toml` as the
    /// root `mind.toml` (removed when empty). Returns the commit sha.
    fn commit(&self, desc: &str, mind_toml: &str) -> String {
        write_file(
            &self.source.join("agents/dev.md"),
            &format!("---\nname: dev\ndescription: {desc}\n---\n# dev\n"),
        );
        let toml = self.source.join("mind.toml");
        if mind_toml.is_empty() {
            let _ = std::fs::remove_file(&toml);
        } else {
            write_file(&toml, mind_toml);
        }
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", desc]);
        self.head()
    }

    fn head(&self) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn tag(&self, name: &str, sha: &str) {
        git(&self.source, &["tag", "-f", name, sha]);
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

    /// A remote-looking spec for the same repo. `mind_env` points git's
    /// `url.<base>.insteadOf` at it, so the clone stays on disk.
    fn remote_spec(&self) -> String {
        format!(
            "{REMOTE_PREFIX}{}",
            self.source.file_name().unwrap().to_string_lossy()
        )
    }

    fn remote_identity(&self) -> String {
        format!(
            "git.example.test/acme/{}",
            self.source.file_name().unwrap().to_string_lossy()
        )
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, &[], None)
    }

    /// `mind` with extra environment (e.g. `MIND_TTY`) and optional stdin. With
    /// `stdin: None` the child gets `/dev/null`.
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_TTY")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.file://{}/.insteadOf", self.base.display()),
            )
            .env("GIT_CONFIG_VALUE_0", REMOTE_PREFIX)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(match stdin {
                Some(_) => Stdio::piped(),
                None => Stdio::null(),
            });
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("run mind");
        if let Some(text) = stdin {
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(text.as_bytes())
                .expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait for mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// The installed `dev` agent's text in the agent home.
    fn installed_agent(&self) -> String {
        std::fs::read_to_string(self.claude_home.join("agents/dev.md")).unwrap_or_default()
    }

    /// The registry entry of this sandbox's source, or `None` when not melded.
    fn entry(&self) -> Option<Value> {
        self.entry_named(&self.identity())
    }

    fn entry_named(&self, identity: &str) -> Option<Value> {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).expect("sources.json is JSON");
        v["sources"]
            .as_array()?
            .iter()
            .find(|s| s["name"].as_str() == Some(identity))
            .cloned()
    }

    /// The recorded pin and commit.
    fn pin(&self) -> (Value, String) {
        pin_of(self.entry().expect("source is melded"))
    }

    /// The recorded pin and commit of the source melded by `remote_spec`.
    fn remote_pin(&self) -> (Value, String) {
        pin_of(
            self.entry_named(&self.remote_identity())
                .expect("remote source is melded"),
        )
    }
}

fn pin_of(e: Value) -> (Value, String) {
    (
        e["pin"].clone(),
        e["commit"].as_str().unwrap_or("").to_string(),
    )
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

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} in {dir:?}");
}

const RELEASE_TAGS: &str = "[source]\nrelease-tags = \"v{version}\"\n";

/// The URL prefix `remote_spec` uses; git rewrites it to the sandbox base.
const REMOTE_PREFIX: &str = "https://git.example.test/acme/";

/// Releases v1.0.0, v1.1.0, v2.0.0 plus a prerelease and a non-semver tag,
/// then a default-branch tip past them. Returns the sandbox and the shas of
/// the three releases.
fn released(name: &str, tip_toml: &str) -> (Sandbox, [String; 3]) {
    let sb = Sandbox::new(name);
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-one", RELEASE_TAGS);
    sb.tag("v1.1.0", &b);
    sb.tag("v1.2", &b);
    let rc = sb.commit("two-rc", RELEASE_TAGS);
    sb.tag("v2.0.0-rc.1", &rc);
    let c = sb.commit("two-zero", RELEASE_TAGS);
    sb.tag("v2.0.0", &c);
    sb.commit("unreleased", tip_toml);
    (sb, [a, b, c])
}

#[test]
fn meld_resolves_the_highest_release_in_range() {
    // spec: VER-30 VER-40 VER-42
    let (sb, [_a, b, _c]) = released("ver-meld", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["kind"], "version", "{pin}");
    assert_eq!(pin["value"]["range"], "^1", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(pin["value"]["tag"], "v1.1.0", "{pin}");
    // The highest stable release, outside the range; the prerelease is not it.
    assert_eq!(pin["value"]["highest"], "2.0.0", "{pin}");
    assert_eq!(commit, b, "the commit the resolved tag points at");
}

#[test]
fn meld_with_no_release_in_range_registers_nothing() {
    // spec: VER-42
    let (sb, _) = released("ver-none", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^3"]);
    assert!(!r.success, "meld must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("no release matches ^3") && r.stderr.contains("highest release is 2.0.0"),
        "{}",
        r.stderr
    );
    assert!(sb.entry().is_none(), "nothing is registered");
}

#[test]
fn a_range_that_does_not_parse_is_a_usage_error() {
    // spec: VER-22
    let (sb, _) = released("ver-bad-range", RELEASE_TAGS);
    for bad in ["version=~1", "version=v1", "version=1.2.3.4", "version="] {
        let r = sb.mind(&["meld", &sb.spec(), "--pin", bad]);
        assert!(!r.success, "{bad}: {}", r.stdout);
        assert!(
            r.stderr.contains("invalid --pin value"),
            "{bad}: {}",
            r.stderr
        );
        assert!(sb.entry().is_none(), "{bad}: nothing is registered");
    }
}

#[test]
fn no_release_tags_uses_the_default_template() {
    // spec: VER-4
    let sb = Sandbox::new("ver-default-template");
    let a = sb.commit("bare", "");
    sb.tag("1.0.0", &a);
    let b = sb.commit("prefixed", "");
    sb.tag("v1.3.0", &b);
    sb.tag("release-1.9.0", &b);
    sb.commit("tip", "");
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.3.0", "{pin}");
    assert_eq!(commit, b);

    // No tag a release at all: the meld fails and registers nothing.
    let sb = Sandbox::new("ver-no-releases");
    let a = sb.commit("only", "");
    sb.tag("stable", &a);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no tag is a release"), "{}", r.stderr);
    assert!(sb.entry().is_none());
}

#[test]
fn the_template_comes_from_the_default_branch() {
    // spec: VER-3
    // The releases themselves declare no template; only the default-branch
    // tip does, with a prefix no default-template tag would match.
    let sb = Sandbox::new("ver-tip-template");
    let a = sb.commit("one", "");
    sb.tag("skills-v1.0.0", &a);
    sb.tag("v1.5.0", &a);
    sb.commit("tip", "[source]\nrelease-tags = \"skills-v{version}\"\n");
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, _) = sb.pin();
    assert_eq!(pin["value"]["tag"], "skills-v1.0.0", "{pin}");
}

#[test]
fn meld_defaults_to_the_caret_of_the_highest_release() {
    // spec: VER-50 VER-52
    let (sb, [_a, _b, c]) = released("ver-default-pin", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(
        r.stderr.contains("follows releases ^2.0.0 (now 2.0.0)")
            && r.stderr.contains("asks before a new major"),
        "the meld says it follows releases: {}",
        r.stderr
    );
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["kind"], "version", "{pin}");
    assert_eq!(pin["value"]["range"], "^2.0.0", "{pin}");
    assert_eq!(pin["value"]["resolved"], "2.0.0", "{pin}");
    assert_eq!(pin["value"]["tag"], "v2.0.0", "{pin}");
    assert_eq!(commit, c, "the release commit, not the default-branch tip");
    assert!(sb.installed_agent().contains("two-zero"));
}

#[test]
fn the_default_pin_skips_prereleases_and_yanked_versions() {
    // spec: VER-50
    let tip = "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"2.0.0\"]\n";
    let (sb, [_a, b, _c]) = released("ver-default-yanked", tip);
    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["value"]["range"], "^1.1.0", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);
}

#[test]
fn the_default_pin_skips_a_release_that_needs_a_newer_mind() {
    // spec: VER-50 VER-41 -- the caret of the highest release this binary
    // meets, not of one it would only warn about.
    let sb = Sandbox::new("ver-default-min-mind");
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("two-zero", NEEDS_NEWER_MIND);
    sb.tag("v2.0.0", &b);
    sb.commit("tip", RELEASE_TAGS);

    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(
        r.stderr.contains("skipped 2.0.0: needs mind 99.0"),
        "{}",
        r.stderr
    );
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["value"]["range"], "^1.0.0", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.0.0", "{pin}");
    assert_eq!(pin["value"]["highest"], "2.0.0", "{pin}");
    assert_eq!(commit, a);
}

#[test]
fn no_stable_release_follows_the_default_branch_and_says_so() {
    // spec: VER-50
    let sb = Sandbox::new("ver-default-rc-only");
    let rc = sb.commit("rc", RELEASE_TAGS);
    sb.tag("v1.0.0-rc.1", &rc);
    let tip = sb.commit("tip", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(
        r.stderr
            .contains("has no stable release; following the default branch"),
        "{}",
        r.stderr
    );
    let (pin, commit) = sb.remote_pin();
    assert_ne!(pin["kind"], "version", "{pin}");
    assert_eq!(commit, tip);
}

#[test]
fn a_local_meld_stays_linked_to_its_working_tree() {
    // spec: VER-50 CLI-27
    let (sb, _) = released("ver-default-local", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_ne!(pin["kind"], "version", "{pin}");
    assert_eq!(commit, sb.head(), "the working-tree HEAD");
    assert!(sb.installed_agent().contains("unreleased"));
}

#[test]
fn an_explicit_pin_or_directive_replaces_the_default() {
    // spec: VER-50 DSC-41
    let (sb, [a, _b, _c]) = released("ver-default-override", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.remote_spec(), "--pin", "tag=v1.0.0", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["kind"], "tag", "{pin}");
    assert_eq!(commit, a);

    let tip = "[source]\nrelease-tags = \"v{version}\"\npin-tag = \"v1.0.0\"\n";
    let (sb, [a, _b, _c]) = released("ver-default-directive", tip);
    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["kind"], "tag", "{pin}");
    assert_eq!(commit, a);
}

#[test]
fn a_pin_version_directive_resolves_and_a_consumer_pin_overrides_it() {
    // spec: VER-51
    let tip = "[source]\nrelease-tags = \"v{version}\"\npin-version = \"^1\"\n";
    let (sb, [a, b, _c]) = released("ver-directive", tip);
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["kind"], "version", "{pin}");
    assert_eq!(pin["value"]["range"], "^1", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);

    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version==1.0.0", "--yes"]);
    assert!(r.success, "re-pin: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "=1.0.0", "{pin}");
    assert_eq!(commit, a);
}

#[test]
fn a_bad_or_conflicting_pin_version_directive_fails_the_meld() {
    // spec: VER-51
    for (name, extra) in [
        ("ver-directive-bad", "pin-version = \"~1\"\n"),
        (
            "ver-directive-conflict",
            "pin-version = \"^1\"\npin-tag = \"v1.0.0\"\n",
        ),
    ] {
        let tip = format!("{RELEASE_TAGS}{extra}");
        let (sb, _) = released(name, &tip);
        let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
        assert!(!r.success, "{name}: {}", r.stdout);
        assert!(r.stderr.contains("mind.toml"), "{name}: {}", r.stderr);
        assert!(sb.entry().is_none(), "{name}: nothing is registered");
    }
}

#[test]
fn sync_moves_to_a_new_compatible_release() {
    // spec: VER-43
    let (sb, [_a, b, c]) = released("ver-sync", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // A new 1.x release, and a 3.0.0 past the range.
    let d = sb.commit("one-two", RELEASE_TAGS);
    sb.tag("v1.2.0", &d);
    let e = sb.commit("three", RELEASE_TAGS);
    sb.tag("v3.0.0", &e);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(commit, d, "moved to the new in-range release");
    assert_eq!(pin["value"]["range"], "^1", "sync never changes the range");
    assert_eq!(pin["value"]["resolved"], "1.2.0", "{pin}");
    assert_eq!(pin["value"]["tag"], "v1.2.0", "{pin}");
    assert_eq!(pin["value"]["highest"], "3.0.0", "{pin}");
    let _ = c;

    // Nothing new: sync stays put.
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    assert_eq!(sb.pin().1, d);
}

/// The `version` recorded on the installed `dev` agent, if any.
fn installed_version(sb: &Sandbox) -> Option<String> {
    let raw = std::fs::read_to_string(sb.mind_home.join("manifest.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).expect("manifest.json is JSON");
    v["items"]["agent:dev"]["version"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn upgrade_reports_the_version_delta_of_a_compatible_move() {
    // spec: VER-60
    let (sb, [_a, _b, _c]) = released("ver-delta", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(installed_version(&sb).as_deref(), Some("1.1.0"));

    let d = sb.commit("one-two", RELEASE_TAGS);
    sb.tag("v1.2.0", &d);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(
        r.stdout.contains("version 1.1.0 -> 1.2.0"),
        "the version delta sits next to the commit delta: {}",
        r.stdout
    );
    assert!(r.stdout.contains("commit "), "{}", r.stdout);
    assert!(sb.installed_agent().contains("one-two"));
    assert_eq!(installed_version(&sb).as_deref(), Some("1.2.0"));
}

#[test]
fn an_item_from_a_non_version_pin_records_no_version() {
    // spec: VER-60
    let (sb, _) = released("ver-delta-none", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "tag=v1.0.0", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(sb.installed_agent().contains("one-zero"));
    assert_eq!(installed_version(&sb), None);
}

fn agent(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {name}\n---\n{body}\n")
}

/// Commit the tree as it stands and tag it.
fn release(sb: &Sandbox, msg: &str, tag: &str) -> String {
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", msg]);
    let sha = sb.head();
    sb.tag(tag, &sha);
    sha
}

#[test]
fn a_compatible_move_that_drops_or_rekinds_an_item_is_warned_about() {
    // spec: VER-65
    let sb = Sandbox::new("ver-looks-breaking");
    write_file(&sb.source.join("mind.toml"), RELEASE_TAGS);
    write_file(&sb.source.join("agents/dev.md"), &agent("dev", "one"));
    write_file(&sb.source.join("agents/gone.md"), &agent("gone", "x"));
    write_file(&sb.source.join("agents/shift.md"), &agent("shift", "x"));
    release(&sb, "one-zero", "v1.0.0");
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );

    write_file(&sb.source.join("agents/dev.md"), &agent("dev", "two"));
    std::fs::remove_file(sb.source.join("agents/gone.md")).unwrap();
    std::fs::remove_file(sb.source.join("agents/shift.md")).unwrap();
    write_file(
        &sb.source.join("rules/shift.md"),
        "---\ndescription: s\n---\n",
    );
    release(&sb, "one-one", "v1.1.0");

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "advisory only: {}", r.stderr);
    let head = format!("{} 1.0.0 -> 1.1.0 looks breaking:", sb.identity());
    assert!(
        r.stdout.contains(&format!("{head} drops agent:gone")),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout
            .contains(&format!("{head} changes shift from agent to rule")),
        "{}",
        r.stdout
    );
    assert!(
        sb.installed_agent().contains("two"),
        "the upgrade went ahead"
    );
    // spec: CLI-245 -- a drop points at the command that clears it.
    assert!(r.stderr.contains("mind forget --dropped"), "{}", r.stderr);
    let f = sb.mind(&["forget", "--dropped", "--yes"]);
    assert!(f.success, "{}", f.stderr);
    assert!(f.stdout.contains("forgot agent:gone"), "{}", f.stdout);
    let again = sb.mind(&["upgrade", "--yes"]);
    assert!(
        !again.stdout.contains("drops agent:gone"),
        "{}",
        again.stdout
    );
}

#[test]
fn a_compatible_move_that_removes_a_token_referent_is_warned_about() {
    // spec: VER-65
    let sb = Sandbox::new("ver-looks-breaking-ref");
    write_file(&sb.source.join("mind.toml"), RELEASE_TAGS);
    write_file(
        &sb.source.join("agents/dev.md"),
        &agent("dev", "use {{ns:helper}}"),
    );
    write_file(&sb.source.join("agents/helper.md"), &agent("helper", "h"));
    release(&sb, "one-zero", "v1.0.0");
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );

    std::fs::remove_file(sb.source.join("agents/helper.md")).unwrap();
    release(&sb, "one-one", "v1.1.0");
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        r.stdout.contains(&format!(
            "{} 1.0.0 -> 1.1.0 looks breaking: removes {{{{ns:helper}}}} referenced by agent:dev",
            sb.identity()
        )),
        "{}",
        r.stdout
    );
}

#[test]
fn a_compatible_move_with_no_break_warns_nothing() {
    // spec: VER-65
    let (sb, _) = released("ver-looks-fine", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let d = sb.commit("one-two", RELEASE_TAGS);
    sb.tag("v1.2.0", &d);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(!r.stdout.contains("looks breaking"), "{}", r.stdout);
}

const NEEDS_NEWER_MIND: &str =
    "[source]\nrelease-tags = \"v{version}\"\nmin-mind-version = \"99.0\"\n";

#[test]
fn resolution_skips_a_release_that_needs_a_newer_mind() {
    // spec: VER-41
    let sb = Sandbox::new("ver-min-mind");
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-one", NEEDS_NEWER_MIND);
    sb.tag("v1.1.0", &b);
    let c = sb.commit("one-two", NEEDS_NEWER_MIND);
    sb.tag("v1.2.0", &c);
    sb.commit("tip", RELEASE_TAGS);

    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    for v in ["1.2.0", "1.1.0"] {
        assert!(
            r.stderr.contains(&format!("skipped {v}: needs mind 99.0")),
            "{v}: {}",
            r.stderr
        );
    }
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.0.0", "{pin}");
    assert_eq!(commit, a);

    // sync re-resolves on the same terms: a newer release that fits wins.
    let d = sb.commit("one-three", RELEASE_TAGS);
    sb.tag("v1.3.0", &d);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.3.0", "{pin}");
    assert_eq!(commit, d);
}

#[test]
fn the_skip_report_strips_control_bytes_from_the_source_name() {
    // spec: VER-41 DSC-95
    let sb = Sandbox::new("ver-skip-\u{202e}der");
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-one", NEEDS_NEWER_MIND);
    sb.tag("v1.1.0", &b);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let line = r
        .stderr
        .lines()
        .find(|l| l.contains("skipped 1.1.0"))
        .unwrap_or_else(|| panic!("no skip report in {:?}", r.stderr));
    assert!(!line.contains('\u{202e}'), "{line:?}");
}

#[test]
fn with_no_release_that_fits_the_highest_is_selected_with_the_warning() {
    // spec: VER-41 DSC-40
    let sb = Sandbox::new("ver-min-mind-none");
    let a = sb.commit("one-zero", NEEDS_NEWER_MIND);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-one", NEEDS_NEWER_MIND);
    sb.tag("v1.1.0", &b);
    sb.commit("tip", RELEASE_TAGS);

    // No terminal and no --yes: the DSC-116 gate refuses the meld.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(sb.entry().is_none());

    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);
}

#[test]
fn a_local_version_pin_is_a_snapshot_of_the_release() {
    // spec: VER-81 CLI-27
    let (sb, [_a, b, _c]) = released("ver-local-snapshot", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(sb.pin().1, b);
    // Read from the sources tree, not the working tree (which is at the tip).
    assert!(sb.installed_agent().contains("one-one"));
    assert!(
        std::fs::read_to_string(sb.source.join("agents/dev.md"))
            .unwrap()
            .contains("unreleased"),
        "the working tree is untouched"
    );
}

#[test]
fn a_local_directory_that_is_not_a_repo_fails_with_the_git_error() {
    // spec: VER-81
    let sb = Sandbox::new("ver-not-a-repo");
    let plain = sb.base.join("plain");
    write_file(&plain.join("agents/dev.md"), &agent("dev", "x"));
    let r = sb.mind(&["meld", &plain.to_string_lossy(), "--pin", "version=^1"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("git"), "{}", r.stderr);
}

#[test]
fn an_item_link_instance_accepts_a_version_pin() {
    // spec: VER-81 LNK-4
    let sb = Sandbox::new("ver-item-link");
    write_file(&sb.source.join("mind.toml"), RELEASE_TAGS);
    let skill = |desc: &str| format!("---\nname: greet\ndescription: {desc}\n---\n# greet\n");
    write_file(&sb.source.join("skills/greet/SKILL.md"), &skill("one-zero"));
    write_file(&sb.source.join("agents/other.md"), &agent("other", "x"));
    let a = release(&sb, "one-zero", "v1.0.0");
    write_file(&sb.source.join("skills/greet/SKILL.md"), &skill("tip"));
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "tip"]);

    let link = format!("file://{}/tree/main/skills/greet", sb.spec());
    let r = sb.mind(&["meld", &link, "--pin", "version=^1", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let e = sb
        .entry_named(&format!("{}#skills/greet", sb.identity()))
        .expect("the link instance is registered");
    let (pin, commit) = pin_of(e);
    assert_eq!(pin["kind"], "version", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.0.0", "{pin}");
    assert_eq!(commit, a);
    let installed =
        std::fs::read_to_string(sb.claude_home.join("skills/greet/SKILL.md")).unwrap_or_default();
    assert!(installed.contains("one-zero"), "{installed}");
    assert!(
        !sb.claude_home.join("agents/other.md").exists(),
        "only the linked item"
    );
}

#[test]
fn a_version_move_never_grants_hook_consent() {
    // spec: VER-70 HOOK-22 HOOK-23
    let sb = Sandbox::new("ver-hooks");
    let log = sb.base.join("update.log");
    let toml = format!(
        "{RELEASE_TAGS}\n[[hooks]]\nrun = \"echo ran >> {}\"\nevent = \"update\"\n",
        log.display()
    );
    let runs = || {
        std::fs::read_to_string(&log)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    };
    let a = sb.commit("one-zero", &toml);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("two-zero", &toml);
    sb.tag("v2.0.0", &b);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );

    // A compatible move under --yes, with no terminal: the hook is skipped.
    let c = sb.commit("one-one", &toml);
    sb.tag("v1.1.0", &c);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert_eq!(sb.pin().1, c);
    assert_eq!(runs(), 0, "--yes is not hook consent: {}", r.stdout);

    // A breaking move taken with --allow-breaking: still skipped.
    let r = sb.mind(&["upgrade", "--allow-breaking", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert_eq!(sb.pin().1, b);
    assert_eq!(runs(), 0, "--allow-breaking is not hook consent");

    // Only the dangerous flag runs it.
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert_eq!(runs(), 1, "{}", r.stdout);
}

#[test]
fn sync_does_not_follow_a_moved_release_tag() {
    // spec: VER-44
    let (sb, [_a, b, _c]) = released("ver-moved", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // Re-point v1.1.0 at a new commit, with no newer 1.x release.
    let moved = sb.commit("moved", RELEASE_TAGS);
    sb.tag("v1.1.0", &moved);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    assert!(
        r.stderr.contains("release tag v1.1.0 moved"),
        "warns about the moved tag: {}",
        r.stderr
    );
    let (pin, commit) = sb.pin();
    assert_eq!(commit, b, "keeps the recorded commit");
    assert_eq!(pin["value"]["tag"], "v1.1.0");

    // Re-melding with --pin accepts the moved tag.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "re-pin: {}", r.stderr);
    assert_eq!(sb.pin().1, moved);
}

/// Assert that the stderr line containing `needle` carries no U+202E.
fn stripped_line(stderr: &str, needle: &str) {
    let line = stderr
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in {stderr:?}"));
    assert!(!line.contains('\u{202e}'), "{line:?}");
}

#[test]
fn release_warnings_and_errors_strip_control_bytes_from_the_source_name() {
    // spec: VER-11 VER-42 VER-44 DSC-95
    // No release-tags: the default template reads both `v1.0.0` and `1.0.0`,
    // here at different commits, so 1.0.0 is ambiguous and skipped.
    let sb = Sandbox::new("ver-warn-\u{202e}der");
    let a = sb.commit("one-zero", "");
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-zero-again", "");
    sb.tag("1.0.0", &b);
    let c = sb.commit("one-one", "");
    sb.tag("v1.1.0", &c);

    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^3"]);
    assert!(!r.success, "meld must fail: {}", r.stdout);
    stripped_line(&r.stderr, "no release matches ^3");

    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    stripped_line(
        &r.stderr,
        "tags 1.0.0 and v1.0.0 name the same version at different commits",
    );
    assert_eq!(sb.pin().1, c);

    let moved = sb.commit("moved", "");
    sb.tag("v1.1.0", &moved);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    stripped_line(&r.stderr, "release tag v1.1.0 moved");
}

#[test]
fn a_yanked_release_is_never_selected() {
    // spec: VER-46
    // The yanked list is read from the default-branch tip.
    let tip = "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"1.1.0\"]\n";
    let (sb, [a, _b, _c]) = released("ver-yanked", tip);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.0.0", "{pin}");
    assert_eq!(commit, a);
}

#[test]
fn a_source_on_a_since_yanked_release_stays_until_a_newer_one() {
    // spec: VER-46
    let (sb, [_a, b, _c]) = released("ver-yanked-later", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(sb.pin().1, b);

    // 1.1.0 is yanked after the source resolved to it: sync does not step
    // down to 1.0.0.
    sb.commit(
        "yank",
        "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"1.1.0\"]\n",
    );
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);

    // A newer in-range release moves it.
    let d = sb.commit(
        "one-two",
        "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"1.1.0\"]\n",
    );
    sb.tag("v1.2.0", &d);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.2.0", "{pin}");
    assert_eq!(commit, d);
}

#[test]
fn re_pinning_switches_between_range_exact_and_other_pins() {
    // spec: VER-80
    let (sb, [a, b, c]) = released("ver-repin", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // Step back to an older release with an exact hold.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version==1.0.0"]);
    assert!(r.success, "exact: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "=1.0.0", "{pin}");
    assert_eq!(commit, a);

    // Over to another pin kind, then back to a range.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "tag=v2.0.0"]);
    assert!(r.success, "tag: {}", r.stderr);
    assert_eq!(sb.pin().0["kind"], "tag");
    assert_eq!(sb.pin().1, c);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^2"]);
    assert!(r.success, "range: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["kind"], "version");
    assert_eq!(pin["value"]["resolved"], "2.0.0", "{pin}");
    assert_eq!(commit, c);
}

#[test]
fn a_version_range_is_unpinned_under_require_pinned_policy() {
    // spec: VER-100 -- a range floats, so POL-20's pinned policy refuses it;
    // an exact version is pinned.
    let (sb, [a, b, _c]) = released("ver-policy", RELEASE_TAGS);
    let policy = sb.base.join("policy.toml");
    write_file(&policy, "[sources]\npinned = true\n");
    let run = |pin: &str| {
        Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(["meld", &sb.spec(), "--pin", pin])
            .env("MIND_HOME", &sb.mind_home)
            .env("CLAUDE_HOME", &sb.claude_home)
            .env("MIND_POLICY_FILE", &policy)
            .stdin(Stdio::null())
            .output()
            .expect("run mind")
    };
    let out = run("version=^1");
    assert!(!out.status.success(), "a range must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("must be pinned"), "{stderr}");
    assert!(sb.entry().is_none());
    let out = run("version==1.0.0");
    assert!(
        out.status.success(),
        "an exact version is pinned: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(sb.pin().1, a);

    // With allow-version-ranges, a range counts as pinned.
    write_file(
        &policy,
        "[sources]\npinned = true\nallow-version-ranges = true\n",
    );
    let out = run("version=^1");
    assert!(
        out.status.success(),
        "a range is pinned under allow-version-ranges: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "^1", "{pin}");
    assert_eq!(commit, b);
    // A branch still is not.
    let out = run("branch=main");
    assert!(!out.status.success(), "a branch stays unpinned");
}

#[test]
fn a_default_release_pin_the_policy_refuses_is_not_announced() {
    // spec: VER-52 VER-100 -- the note follows the policy check, so a refused
    // meld never claims to follow releases.
    let (sb, _) = released("ver-policy-note", RELEASE_TAGS);
    let policy = sb.base.join("policy.toml");
    write_file(&policy, "[sources]\npinned = true\n");
    let policy_env = policy.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["meld", &sb.remote_spec(), "--yes"],
        &[("MIND_POLICY_FILE", policy_env.as_str())],
        None,
    );
    assert!(!r.success, "the default range is refused: {}", r.stdout);
    assert!(r.stderr.contains("must be pinned"), "{}", r.stderr);
    assert!(!r.stderr.contains("follows releases"), "{}", r.stderr);
}

#[test]
fn sync_upgrade_json_reports_the_passes_breaking_moves() {
    // spec: VER-91
    let (sb, _) = released("ver-sync-upgrade-json", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind(&["sync", "--upgrade", "--yes", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("sync JSON");
    assert_eq!(v["action"], "sync", "{v}");
    assert_eq!(v["sources"][0]["version"]["resolved"], "1.1.0", "{v}");
    assert_eq!(v["breaking"][0]["to"], "2.0.0", "{v}");
    assert_eq!(v["breaking"][0]["applied"], false, "{v}");
}

#[test]
fn a_policy_auto_meld_entry_takes_a_version_range() {
    // spec: VER-101
    let (sb, [_a, b, _c]) = released("ver-auto-meld", RELEASE_TAGS);
    let policy = sb.base.join("policy.toml");
    write_file(
        &policy,
        &format!(
            "[sources]\npinned = true\nallow-version-ranges = true\n\n\
             [[sources.auto_meld]]\nrepo = \"{}\"\nversion = \"^1\"\n",
            sb.spec()
        ),
    );
    let sync = || {
        Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(["sync"])
            .env("MIND_HOME", &sb.mind_home)
            .env("CLAUDE_HOME", &sb.claude_home)
            .env("MIND_POLICY_FILE", &policy)
            .stdin(Stdio::null())
            .output()
            .expect("run mind")
    };
    let out = sync();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "^1", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);

    // The recorded resolution is not drift from the declared range.
    let out = sync();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("re-pinned"), "{stdout}");
    assert_eq!(sb.pin().0["value"]["resolved"], "1.1.0");
}

#[test]
fn review_checks_release_tags_and_breaks_since_the_highest_release() {
    // spec: VER-130
    let (sb, _) = released("ver-review", RELEASE_TAGS);
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // `v1.2` fits `v{version}` but is not semver.
    assert!(
        r.stdout
            .contains("advisory [non-semver-release-tag]: tag v1.2 fits"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("apparent-break"), "{}", r.stdout);

    // The tree since v2.0.0 turns the dev agent into a rule.
    std::fs::remove_file(sb.source.join("agents/dev.md")).unwrap();
    write_file(
        &sb.source.join("rules/dev.md"),
        "---\ndescription: d\n---\n",
    );
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory only: {}", r.stderr);
    assert!(
        r.stdout.contains(
            "advisory [apparent-break]: since release 2.0.0 (v2.0.0), this tree changes dev from agent to rule"
        ),
        "{}",
        r.stdout
    );

    // A template that matches no tag.
    let bare = Sandbox::new("ver-review-none");
    bare.commit("one", RELEASE_TAGS);
    let r = bare.mind(&["review", &bare.spec()]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("advisory [no-release-tags]"),
        "{}",
        r.stdout
    );

    // A template that does not parse is a hard finding.
    bare.commit("two", "[source]\nrelease-tags = \"v{major}\"\n");
    let r = bare.mind(&["review", &bare.spec()]);
    assert!(!r.success, "{}", r.stdout);
    assert!(
        r.stderr.contains("error [toml-parse-error]") && r.stderr.contains("release-tags"),
        "{}",
        r.stderr
    );
}

#[test]
fn review_reports_a_dropped_item_and_tags_it_cannot_list() {
    // spec: VER-130
    let sb = Sandbox::new("ver-review-drop");
    write_file(&sb.source.join("mind.toml"), RELEASE_TAGS);
    write_file(&sb.source.join("agents/dev.md"), &agent("dev", "d"));
    write_file(&sb.source.join("agents/gone.md"), &agent("gone", "g"));
    release(&sb, "one-zero", "v1.0.0");
    std::fs::remove_file(sb.source.join("agents/gone.md")).unwrap();
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory only: {}", r.stderr);
    assert!(
        r.stdout.contains(
            "advisory [apparent-break]: since release 1.0.0 (v1.0.0), this tree drops agent:gone"
        ),
        "{}",
        r.stdout
    );

    // A directory that is not a git repo has no tags to list.
    let plain = sb.base.join("plain");
    write_file(&plain.join("mind.toml"), RELEASE_TAGS);
    write_file(&plain.join("agents/dev.md"), &agent("dev", "d"));
    let r = sb.mind(&["review", &plain.to_string_lossy()]);
    assert!(r.success, "advisory only: {}", r.stderr);
    assert!(
        r.stdout.contains("advisory [release-tags-unchecked]"),
        "{}",
        r.stdout
    );
}

#[test]
fn init_source_declares_release_tags_when_the_repo_has_releases() {
    // spec: VER-131
    let sb = Sandbox::new("ver-init");
    let a = sb.commit("one", "");
    sb.tag("not-a-release", &a);
    let r = sb.mind(&["init-source", &sb.spec()]);
    assert!(r.success, "{}", r.stderr);
    let toml = std::fs::read_to_string(sb.source.join("mind.toml")).unwrap();
    assert!(!toml.contains("release-tags"), "{toml}");

    std::fs::remove_file(sb.source.join("mind.toml")).unwrap();
    sb.tag("v1.0.0", &a);
    let r = sb.mind(&["init-source", &sb.spec()]);
    assert!(r.success, "{}", r.stderr);
    let toml = std::fs::read_to_string(sb.source.join("mind.toml")).unwrap();
    assert!(toml.contains("release-tags = \"v{version}\""), "{toml}");
    assert!(r.stdout.contains("1 release tag(s)"), "{}", r.stdout);

    // An existing mind.toml is left as it is.
    write_file(&sb.source.join("mind.toml"), "[source]\n");
    assert!(sb.mind(&["init-source", &sb.spec()]).success);
    let toml = std::fs::read_to_string(sb.source.join("mind.toml")).unwrap();
    assert_eq!(toml, "[source]\n");
}

#[test]
fn recall_suggests_a_version_pin_for_a_release_tag_pin() {
    // spec: VER-82
    let (sb, _) = released("ver-tag-hint", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "tag=v1.0.0", "--yes"])
            .success
    );
    let hint = "--pin version=^1.0.0";
    for args in [vec!["recall", "--sources"], vec!["recall"]] {
        let r = sb.mind(&args);
        assert!(r.success, "{args:?}: {}", r.stderr);
        let line = r
            .stdout
            .lines()
            .find(|l| l.contains(hint))
            .unwrap_or_else(|| panic!("{args:?}: no hint in {}", r.stdout));
        assert!(line.contains(&sb.identity()), "{line}");
        assert!(line.contains("mind meld"), "{line}");
    }

    // A tag that is not a release under the template gets no suggestion.
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "tag=v1.2", "--yes"])
            .success
    );
    let r = sb.mind(&["recall", "--sources"]);
    assert!(!r.stdout.contains("--pin version="), "{}", r.stdout);

    // Nor does a source already on a version pin.
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind(&["recall", "--sources"]);
    assert!(!r.stdout.contains("--pin version="), "{}", r.stdout);
}

#[test]
fn the_version_pin_hint_strips_control_bytes_from_the_source_name() {
    // spec: VER-82 DSC-95 -- the suggested command quotes a sanitized name.
    // A repo spec refuses control characters, but not a bidi override.
    let (sb, _) = released("ver-hint-\u{202e}der", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "tag=v1.0.0", "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.success, "{}", r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("--pin version=^1.0.0"))
        .unwrap_or_else(|| panic!("no hint in {:?}", r.stdout));
    assert!(!line.contains('\u{202e}'), "{line:?}");
}

#[test]
fn dump_ranges_emits_the_range_and_plain_dump_still_freezes() {
    // spec: VER-120
    let (sb, [_a, b, _c]) = released("ver-dump", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );

    let plain = sb.mind(&["dump"]);
    assert!(plain.success, "{}", plain.stderr);
    assert!(
        plain.stdout.contains(&format!("pin-ref = \"{b}\"")),
        "{}",
        plain.stdout
    );
    assert!(!plain.stdout.contains("pin-version"), "{}", plain.stdout);

    let ranged = sb.mind(&["dump", "--ranges"]);
    assert!(ranged.success, "{}", ranged.stderr);
    assert!(
        ranged.stdout.contains("pin-version = \"^1\""),
        "{}",
        ranged.stdout
    );
    assert!(!ranged.stdout.contains("pin-ref"), "{}", ranged.stdout);

    // The output re-melds: the nested source resolves its range afresh.
    let dumped = sb.base.join("dumped");
    write_file(&dumped.join("mind.toml"), &ranged.stdout);
    for args in [
        vec!["-c", "init.defaultBranch=main", "init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "dump"],
    ] {
        git(&dumped, &args);
    }
    let fresh = sb.base.join("mind-fresh");
    let fresh_home = fresh.to_string_lossy().into_owned();
    let fresh_lobe = sb.base.join("claude-fresh").to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["meld", &dumped.to_string_lossy(), "--yes"],
        &[("MIND_HOME", &fresh_home), ("CLAUDE_HOME", &fresh_lobe)],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let raw = std::fs::read_to_string(fresh.join("sources.json")).unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();
    let nested = doc["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == sb.identity())
        .unwrap_or_else(|| panic!("nested source not registered: {doc}"));
    assert_eq!(nested["pin"]["value"]["range"], "^1", "{nested}");
    assert_eq!(nested["commit"], b.as_str(), "{nested}");
}

#[test]
fn upgrade_lists_a_breaking_release_without_applying_it() {
    // spec: VER-61
    let (sb, [_a, b, _c]) = released("ver-break-list", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    assert!(
        sb.installed_agent().contains("one-one"),
        "{}",
        sb.installed_agent()
    );
    for args in [vec!["upgrade"], vec!["upgrade", "--yes"]] {
        // spec: VER-63 -- no terminal and no --allow-breaking, and --yes alone,
        // both list the move and skip it.
        let r = sb.mind(&args);
        assert!(r.success, "{args:?}: {}", r.stderr);
        assert!(
            r.stdout.contains("1.1.0 -> 2.0.0 (breaking, not applied"),
            "{args:?}: {}",
            r.stdout
        );
        let (pin, commit) = sb.pin();
        assert_eq!(pin["value"]["range"], "^1", "{pin}");
        assert_eq!(commit, b);
        assert!(sb.installed_agent().contains("one-one"));
    }
}

#[test]
fn recall_shows_the_release_and_marks_breaking_and_yanked() {
    // spec: VER-90
    let (sb, _) = released("ver-recall-marks", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let take = format!(
        "breaking release 2.0.0 available; take it with `mind upgrade --allow-breaking --source '{}'`",
        sb.identity()
    );
    for args in [vec!["recall", "--sources"], vec!["recall"]] {
        let r = sb.mind(&args);
        assert!(r.success, "{args:?}: {}", r.stderr);
        assert!(
            r.stdout.contains("release 1.1.0 (^1)"),
            "{args:?}: {}",
            r.stdout
        );
        assert!(r.stdout.contains(&take), "{args:?}: {}", r.stdout);
        assert!(!r.stdout.contains("yanked"), "{args:?}: {}", r.stdout);
    }

    // 1.1.0 is yanked after the source resolved to it: sync records it and
    // recall marks it without a fetch.
    sb.commit(
        "yank",
        "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"1.1.0\"]\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.stdout.contains("release 1.1.0 is yanked"), "{}", r.stdout);
}

#[test]
fn the_breaking_release_lines_strip_control_bytes_from_the_source_name() {
    // spec: VER-61 VER-90 DSC-95
    let (sb, _) = released("ver-break-\u{202e}der", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind(&["upgrade"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    stripped_line(&r.stdout, "(breaking, not applied");
    let r = sb.mind(&["recall", "--sources"]);
    stripped_line(&r.stdout, "breaking release 2.0.0 available");
}

#[test]
fn json_reports_each_version_pinned_source_and_the_breaking_moves() {
    // spec: VER-91
    let (sb, _) = released("ver-json", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let expected = serde_json::json!({
        "range": "^1",
        "resolved": "1.1.0",
        "tag": "v1.1.0",
        "highest_available": "2.0.0",
        "breaking_available": true,
        "yanked": false,
    });

    let r = sb.mind(&["recall", "--sources", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("recall JSON");
    assert_eq!(v["items"][0]["version"], expected, "{v}");

    let r = sb.mind(&["sync", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("sync JSON");
    assert_eq!(v["sources"][0]["source"], sb.identity().as_str(), "{v}");
    assert_eq!(v["sources"][0]["version"], expected, "{v}");

    let r = sb.mind(&["upgrade", "--yes", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("upgrade JSON");
    assert_eq!(v["sources"][0]["version"], expected, "{v}");
    assert_eq!(
        v["breaking"],
        serde_json::json!([{
            "source": sb.identity(),
            "from": "1.1.0",
            "to": "2.0.0",
            "applied": false,
        }]),
        "{v}"
    );

    let r = sb.mind(&["upgrade", "--allow-breaking", "--yes", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("upgrade JSON");
    assert_eq!(v["breaking"][0]["applied"], true, "{v}");
    assert_eq!(v["sources"][0]["version"]["range"], "^2.0.0", "{v}");
    assert_eq!(
        v["sources"][0]["version"]["breaking_available"], false,
        "{v}"
    );
}

#[test]
fn allow_breaking_takes_the_new_major_and_upgrades_its_items() {
    // spec: VER-63
    let (sb, [_a, _b, c]) = released("ver-break-allow", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind(&["upgrade", "--allow-breaking", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(
        pin["value"]["range"], "^2.0.0",
        "the range moves to the new major"
    );
    assert_eq!(pin["value"]["resolved"], "2.0.0", "{pin}");
    assert_eq!(commit, c);
    assert!(
        sb.installed_agent().contains("two-zero"),
        "items upgrade through the ordinary pass: {}",
        sb.installed_agent()
    );
    // Nothing breaking is left to offer.
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(!r.stdout.contains("breaking"), "{}", r.stdout);
}

#[test]
fn a_terminal_prompts_for_each_breaking_release() {
    // spec: VER-62
    let (sb, [_a, b, c]) = released("ver-break-prompt", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );

    // Declined (the default): listed, not applied.
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(r.stdout.contains("breaking, not applied"), "{}", r.stdout);
    assert_eq!(sb.pin().1, b);

    // Accepted, then the ordinary upgrade prompt for its items.
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("y\ny\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "^2.0.0", "{pin}");
    assert_eq!(commit, c);
    assert!(
        sb.installed_agent().contains("two-zero"),
        "{}",
        sb.installed_agent()
    );
}

#[test]
fn the_breaking_prompt_shows_an_annotated_tags_notes() {
    // spec: VER-64
    let (sb, [_a, b, _c]) = released("ver-break-notes", RELEASE_TAGS);
    let d = sb.commit("three-zero", RELEASE_TAGS);
    git(
        &sb.source,
        &[
            "tag",
            "-a",
            "v3.0.0",
            "-m",
            "Drop the dev agent's \x1b[31mold\x1b[0m flags\n\nSee the migration notes.",
            &d,
        ],
    );
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(r.stderr.contains("notes for v3.0.0:"), "{}", r.stderr);
    assert!(
        r.stderr.contains("Drop the dev agent's old flags"),
        "sanitized: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("See the migration notes."),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains('\x1b'), "no escape reaches the terminal");
    assert_eq!(sb.pin().1, b, "declined");
}

#[test]
fn a_lightweight_breaking_tag_shows_no_notes() {
    // spec: VER-64
    let (sb, _) = released("ver-break-no-notes", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(!r.stderr.contains("notes for"), "{}", r.stderr);
}

#[test]
fn the_breaking_prompt_caps_long_notes_at_twenty_lines() {
    // spec: VER-64
    let (sb, _) = released("ver-break-notes-cap", RELEASE_TAGS);
    let d = sb.commit("three-zero", RELEASE_TAGS);
    let body: Vec<String> = (2..=30).map(|n| format!("body {n}")).collect();
    let message = format!("Release three\n\n{}", body.join("\n"));
    git(&sb.source, &["tag", "-a", "v3.0.0", "-m", &message, &d]);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    // The subject, the blank line after it, then the body: 20 lines in all.
    assert!(r.stderr.contains("Release three"), "{}", r.stderr);
    assert!(r.stderr.contains("body 19\n"), "{}", r.stderr);
    assert!(!r.stderr.contains("body 20"), "capped: {}", r.stderr);
    assert!(r.stderr.contains("\n  ...\n"), "{}", r.stderr);
}

#[test]
fn the_breaking_prompt_leaves_out_a_signed_tags_signature() {
    // spec: VER-64
    let (sb, _) = released("ver-break-notes-signed", RELEASE_TAGS);
    let key = sb.base.join("signing-key");
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&key)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("ssh-keygen is needed to sign a test tag");
    assert!(status.success(), "ssh-keygen failed");
    let d = sb.commit("three-zero", RELEASE_TAGS);
    let signing_key = format!("user.signingkey={}", key.display());
    git(
        &sb.source,
        &[
            "-c",
            "gpg.format=ssh",
            "-c",
            &signing_key,
            "tag",
            "-s",
            "v3.0.0",
            "-m",
            "Signed release\n\nSee the migration notes.",
            &d,
        ],
    );
    let tag_object = Command::new("git")
        .args(["cat-file", "tag", "v3.0.0"])
        .current_dir(&sb.source)
        .output()
        .expect("read the tag object");
    assert!(
        String::from_utf8_lossy(&tag_object.stdout).contains("BEGIN SSH SIGNATURE"),
        "the tag is signed"
    );
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(r.stderr.contains("notes for v3.0.0:"), "{}", r.stderr);
    assert!(r.stderr.contains("Signed release"), "{}", r.stderr);
    assert!(
        r.stderr.contains("See the migration notes."),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("SIGNATURE"), "{}", r.stderr);
}

#[test]
fn a_plain_remote_meld_follows_compatible_releases_and_takes_a_new_major_on_consent() {
    // spec: VER-50 VER-52 VER-43 VER-60 VER-61 VER-63 VER-90
    let sb = Sandbox::new("ver-lifecycle");
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    sb.commit("unreleased", RELEASE_TAGS);

    // A plain meld pins the caret of the only release.
    let r = sb.mind(&["meld", &sb.remote_spec(), "--yes"]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(
        r.stderr.contains("follows releases ^1.0.0 (now 1.0.0)"),
        "{}",
        r.stderr
    );
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["value"]["range"], "^1.0.0", "{pin}");
    assert_eq!(commit, a);
    assert!(sb.installed_agent().contains("one-zero"));

    // A compatible release: upgrade syncs to it and installs it.
    let b = sb.commit("one-one", RELEASE_TAGS);
    sb.tag("v1.1.0", &b);
    sb.commit("unreleased", RELEASE_TAGS);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(r.stdout.contains("version 1.0.0 -> 1.1.0"), "{}", r.stdout);
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["value"]["range"], "^1.0.0", "the range never moves");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(commit, b);
    assert!(sb.installed_agent().contains("one-one"));

    // A new major: listed and left by upgrade, marked by recall.
    let c = sb.commit("two-zero", RELEASE_TAGS);
    sb.tag("v2.0.0", &c);
    sb.commit("unreleased", RELEASE_TAGS);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(
        r.stdout.contains("1.1.0 -> 2.0.0 (breaking, not applied"),
        "{}",
        r.stdout
    );
    assert_eq!(sb.remote_pin().1, b);
    assert!(sb.installed_agent().contains("one-one"));
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.stdout.contains("release 1.1.0 (^1.0.0)"), "{}", r.stdout);
    assert!(
        r.stdout.contains("breaking release 2.0.0 available"),
        "{}",
        r.stdout
    );

    // Taken on consent: the range moves to the new major and its items upgrade.
    let r = sb.mind(&["upgrade", "--allow-breaking", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    let (pin, commit) = sb.remote_pin();
    assert_eq!(pin["value"]["range"], "^2.0.0", "{pin}");
    assert_eq!(pin["value"]["resolved"], "2.0.0", "{pin}");
    assert_eq!(commit, c);
    assert!(sb.installed_agent().contains("two-zero"));
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.stdout.contains("release 2.0.0 (^2.0.0)"), "{}", r.stdout);
    assert!(!r.stdout.contains("breaking"), "{}", r.stdout);
}

#[test]
fn json_on_a_terminal_lists_a_breaking_release_without_prompting() {
    // spec: VER-63 LIFE-45
    let (sb, [_a, b, _c]) = released("ver-break-json-tty", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"])
            .success
    );
    let r = sb.mind_env(&["upgrade", "--json"], &[("MIND_TTY", "1")], Some("y\n"));
    assert!(r.success, "upgrade: {}", r.stderr);
    let v: Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is one JSON document ({e}): {}", r.stdout));
    assert_eq!(v["breaking"][0]["applied"], false, "{v}");
    assert_eq!(sb.pin().1, b, "the reply on stdin is never read");
}

#[test]
fn an_exact_hold_is_never_offered_a_breaking_release() {
    // spec: VER-61
    let (sb, [a, _b, _c]) = released("ver-break-exact", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version==1.0.0", "--yes"])
            .success
    );
    let r = sb.mind(&["upgrade", "--allow-breaking", "--yes"]);
    assert!(r.success, "upgrade: {}", r.stderr);
    assert!(!r.stdout.contains("breaking"), "{}", r.stdout);
    assert_eq!(sb.pin().1, a);
}
