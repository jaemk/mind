//! Integration tests for the `install-items` subset directive (DSC-62/63/64)
//! and the consumer-side `meld --learn <glob>` subset install (CLI-236).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. No network. The fixture mirrors the pattern in tests/cli.rs.

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
    fn build(name: &str, with_fixture: bool) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-install-items-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        if with_fixture {
            write_file(
                &source.join("skills/review/SKILL.md"),
                "---\nname: review\ndescription: Review the diff\n---\n# review\n",
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

    /// A source repo with items (one skill, one agent, one rule).
    fn new(name: &str) -> Sandbox {
        Sandbox::build(name, true)
    }

    /// A source repo with no items (e.g. a pure super-source).
    fn bare(name: &str) -> Sandbox {
        Sandbox::build(name, false)
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The registered identity `mind` derives for this repo when melded as a
    /// plain (non-item-link) local source: `local/<base>/<repo>` (LNK-4/STO-11).
    fn identity(&self) -> String {
        format!(
            "local/{}/{}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
    }

    /// A `file://` item link into this sandbox's source repo (LNK-1).
    fn link(&self, tail: &str) -> String {
        format!("file://{}/{tail}", self.source.to_string_lossy())
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, &[], None)
    }

    /// `mind` with extra environment (e.g. `MIND_TTY`, HOOK-109) and optional
    /// stdin text, so the interactive CLI-23 gate `--learn` reuses is drivable
    /// from a test. With `stdin: None` the child gets `/dev/null`, whose EOF
    /// `read_confirm` treats as "no".
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
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

    /// Write a file under the source repo and commit it.
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write_file(&self.source.join(rel), contents);
        git_commit(&self.source);
    }

    /// Write a `len`-byte sparse file under the source repo and commit it.
    /// Used to build an oversized metadata fixture (DSC-91) without the test
    /// itself allocating a `len`-byte buffer: `File::set_len` creates the file
    /// at the requested size on disk (a hole, materialized as zero bytes on
    /// read) rather than writing `len` bytes from memory.
    fn write_sparse_and_commit(&self, rel: &str, len: u64) {
        let path = self.source.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(len).unwrap();
        drop(file);
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

/// The text between the first pair of backticks: the command mind printed for
/// the user to paste. Tests run it VERBATIM, so a suggestion that would not
/// work when pasted fails the test rather than passing on a substring match.
fn backticked(text: &str) -> String {
    let start = text.find('`').unwrap_or_else(|| {
        panic!("expected a backticked command in: {text}");
    });
    let rest = &text[start + 1..];
    let end = rest.find('`').expect("a closing backtick");
    rest[..end].to_string()
}

/// Split a single-quoted shell command line into argv. Mirrors the splitter in
/// tests/cli_item_link.rs; an integration test is its own crate and cannot
/// reach the binary's own quoting helpers.
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

/// Run a printed `mind ...` command string through the real binary.
/// Run a command mind printed for the user to paste, exactly as printed.
///
/// A printed fallback may be a `&&`-joined SEQUENCE (`learn` takes one
/// positional, so several matches are several commands); each is run in turn
/// and the first failure is returned, which is what `&&` means in a shell.
fn run_printed(sb: &Sandbox, command: &str) -> Run {
    let mut last: Option<Run> = None;
    for step in command.split("&&") {
        let argv = shell_split(step.trim());
        assert_eq!(
            argv.first().map(String::as_str),
            Some("mind"),
            "every step of the printed command must invoke mind: {command}"
        );
        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        let run = sb.mind(&args);
        if !run.success {
            return run;
        }
        last = Some(run);
    }
    last.expect("a printed command must have at least one step")
}

/// Count the melded sources by reading sources.json (0 when absent).
fn source_count(sb: &Sandbox) -> usize {
    let path = sb.mind_home.join("sources.json");
    let Ok(json) = std::fs::read_to_string(&path) else {
        return 0;
    };
    json.matches("\"url\"").count()
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

// ----- DSC-62: subset install via install-items -----

#[test]
fn install_items_subset_is_offered_not_others() {
    // spec: DSC-62 — melding a super-source with install-items = ["skill:review"]
    // offers only the named item for install (via --yes auto-install); the other
    // items of the nested source are registered but not installed.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // Meld with --yes: the listed subset should install.
    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The named item is installed.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review (in install-items) must be installed: {:?}",
        registry.claude_home
    );

    // The non-listed items are NOT installed.
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev (not in install-items) must NOT be installed"
    );
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "rule:style (not in install-items) must NOT be installed"
    );

    // But the non-listed items are still available (can be learned explicitly).
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("agent:dev"),
        "agent:dev must still be available via probe: {}",
        probe.stdout
    );
}

#[test]
fn install_items_other_items_remain_available_and_learnable() {
    // spec: DSC-62 — the source's non-listed items stay registered and available;
    // they can be learned explicitly after the super-source is melded.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // Explicitly learn a non-listed item.
    let learn = registry.mind(&["learn", "agent:dev"]);
    assert!(
        learn.success,
        "explicitly learning a non-listed item must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed after explicit `learn`"
    );
}

// ----- CLI-236: `meld --learn <glob>` installs only the matching subset -----

#[test]
fn meld_learn_installs_only_the_matching_items() {
    // spec: CLI-236 -- `--learn <name>` replaces the CLI-23 install-all offer
    // with a subset install scoped to the source being melded.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review", "--yes"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);

    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the matched item must be installed: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "an unmatched item must NOT be installed: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("rules/style.md").exists(),
        "an unmatched item must NOT be installed: {}",
        r.stdout
    );

    // The source is fully melded, so the rest stays available.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:style"),
        "the unmatched items must still be offered: {}",
        probe.stdout
    );
}

#[test]
fn meld_learn_is_repeatable_and_accepts_a_glob() {
    // spec: CLI-236 -- the flag is repeatable and each value may be a glob.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--learn",
        "rev*",
        "--learn",
        "rule:style",
        "--yes",
    ]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("rules/style.md").exists(),
        "both patterns must install: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "an unmatched item must NOT be installed: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_matches_a_prefixed_source_by_bare_name() {
    // spec: CLI-236 -- a pattern matches the BARE name as well as the effective
    // one. At meld time the consumer has not necessarily seen the source's
    // declared prefix (CLI-24 may prompt for it inside this very command), so
    // requiring `team:review` would break the first-run case the flag is for.
    let sb = Sandbox::new("lib");
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"team\"\n");

    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review", "--yes"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/team:review").exists(),
        "the bare name must match the item that installs prefixed: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !sb.claude_home.join("rules/team:style.md").exists(),
        "an unmatched item must NOT be installed: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_matches_a_prefixed_source_by_effective_name() {
    // spec: CLI-236 -- the effective (prefixed) name resolves too, so a user who
    // knows the prefix is not forced to drop it.
    let sb = Sandbox::new("lib");
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"team\"\n");

    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "team:review", "--yes"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/team:review").exists(),
        "the effective name must match: {} {}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn meld_learn_pattern_matching_nothing_is_an_error() {
    // spec: CLI-236 -- a pattern the user named explicitly must match
    // something; unlike the install-all offer, a miss is not "nothing to do".
    // The source stays melded (the failure is in the install pass).
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "nope*", "--yes"]);
    assert!(!r.success, "a non-matching pattern must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("nope*"),
        "the error must name the pattern: {}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("lib"),
        "the source stays melded: {}",
        sources.stdout
    );
}

#[test]
fn meld_learn_rejects_a_source_qualified_pattern() {
    // spec: CLI-236 -- `--learn` selects within the source being melded, so a
    // `#`-carrying value is a usage error rather than a ref resolved elsewhere.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--learn",
        "other/repo#review",
        "--yes",
    ]);
    assert!(!r.success, "a qualified pattern must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("selects within the source being melded"),
        "the error must explain the scoping: {}",
        r.stderr
    );
}

#[test]
fn meld_learn_conflicts_with_register_only() {
    // spec: CLI-236 -- the two flags ask for opposite things.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--register-only",
        "--learn",
        "review",
    ]);
    assert!(
        !r.success && r.stderr.contains("cannot be used with"),
        "--learn with --register-only must be a usage error: {} {}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn meld_learn_pulls_in_the_dependency_closure() {
    // spec: CLI-236 -- a `--learn` match installs through the ordinary learn
    // path, so its intra-source dependencies (DEP-30) come with it.
    let sb = Sandbox::new("lib");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\nrequires: agent:dev\n---\n# review\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review", "--yes"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("agents/dev.md").exists(),
        "the required sibling must be pulled in: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("rules/style.md").exists(),
        "an unrelated item must NOT be installed: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_rejects_an_unusable_pattern_before_registering_the_source() {
    // spec: CLI-236 -- the pattern grammar is checked before the clone, so a
    // typo does not leave a registered source behind. Each rejected form names
    // its own reason rather than collapsing into one blunt message.
    let sb = Sandbox::new("lib");
    for (pattern, expect) in [
        ("", "empty"),
        ("[bad", "not a valid glob"),
        (
            "other/repo#review",
            "selects within the source being melded",
        ),
    ] {
        let r = sb.mind(&["meld", &sb.source_spec(), "--learn", pattern, "--yes"]);
        assert!(
            !r.success,
            "pattern {pattern:?} must be refused: {}",
            r.stdout
        );
        assert!(
            r.stderr.contains(expect),
            "pattern {pattern:?} must report {expect:?}: {}",
            r.stderr
        );
        assert_eq!(
            source_count(&sb),
            0,
            "pattern {pattern:?} must be refused before the source is registered"
        );
    }
}

#[test]
fn meld_learn_no_match_names_the_source_and_the_escapes() {
    // spec: CLI-236 -- a no-match is scoped to the one source searched, not the
    // generic across-all-sources ItemNotFound, and names the two things that
    // most often explain it.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "nope*", "--yes"]);
    assert!(!r.success, "a non-matching pattern must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("matches no item in source")
            && r.stderr.contains("mind probe")
            && r.stderr.contains("--no-tui")
            && r.stderr.contains("--add-root"),
        "the error must be source-scoped and name the escapes: {}",
        r.stderr
    );
}

#[test]
fn meld_learn_without_yes_off_a_tty_installs_nothing_and_says_how() {
    // spec: CLI-236 -- the gate around the batch is the CLI-23 meld gate: with
    // no TTY and no --yes, nothing installs and the note says how to install
    // later. Every integration test is non-TTY, so this is the path a scripted
    // user actually hits.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "nothing installs without --yes off a TTY: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("registered only, nothing installed")
            && r.stdout.contains("mind learn")
            && r.stdout.contains("--yes"),
        "the note must say how to install later: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_reports_already_installed_once_for_the_batch() {
    // spec: CLI-236 CLI-157 -- when every match is already installed, that is
    // reported once for the batch, not once per pattern.
    let sb = Sandbox::new("lib");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--learn", "review", "--yes"])
            .success
    );
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--learn",
        "review",
        "--learn",
        "skill:*",
        "--yes",
    ]);
    assert!(
        r.success,
        "re-meld --learn failed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        r.stdout.matches("already installed").count(),
        1,
        "the already-installed report must appear once for the batch: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_notes_that_it_ignores_recursive() {
    // spec: CLI-236 -- `--learn` scopes to the melded source's own items, so the
    // curated chain is not walked and an explicit `--recursive` has nothing to
    // act on. Name it rather than dropping it silently (the CLI-206 discipline).
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true }}]\n",
            nested.source_spec()
        ),
    );
    registry.write_and_commit(
        "skills/curated/SKILL.md",
        "---\ndescription: A curator's own skill\n---\n# curated\n",
    );

    let r = registry.mind(&[
        "meld",
        &registry.source_spec(),
        "--learn",
        "curated",
        "--recursive",
        "--yes",
    ]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("--recursive ignored"),
        "the ignored flag must be named: {}",
        r.stdout
    );
    assert!(
        registry.claude_home.join("skills/curated").exists(),
        "the named item installs: {}",
        r.stdout
    );
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "the curated chain must NOT be installed under --learn: {}",
        r.stdout
    );
    // The nested source is still registered, only its install pass is skipped.
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("nested"),
        "the nested source must still be registered: {}",
        sources.stdout
    );
}

#[test]
fn meld_learn_json_skips_the_curated_chain_for_a_super_source() {
    // spec: CLI-236 -- the `--json --learn` twin of
    // `meld_learn_notes_that_it_ignores_recursive`: the curated chain of a
    // super-source with a nested `install = true` entry is not walked when
    // `--learn` is given, in JSON mode too. Both of the pre-existing
    // `--json --learn` tests meld a plain (mind.toml-less) source, so the
    // curated-chain walk is a no-op in them either way -- this fixture (a
    // curator with a real nested source to install) is the one that would
    // notice the `if learn_patterns.is_empty()` guard around that walk being
    // removed: without it, this call would install `nested`'s items too.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true }}]\n",
            nested.source_spec()
        ),
    );
    registry.write_and_commit(
        "skills/curated/SKILL.md",
        "---\ndescription: A curator's own skill\n---\n# curated\n",
    );

    let r = registry.mind(&[
        "--json",
        "meld",
        &registry.source_spec(),
        "--learn",
        "curated",
        "--yes",
    ]);
    assert!(
        r.success,
        "meld --learn --json failed: {} {}",
        r.stdout, r.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    let installed = doc["installed"].as_array().expect("installed array");
    assert_eq!(
        installed
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["skill:curated"],
        "only the matched (curator's own) item is reported installed, not the \
         nested source's items: {}",
        r.stdout
    );
    assert!(
        registry.claude_home.join("skills/curated").exists(),
        "the matched item must actually install: {}",
        r.stdout
    );
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "the nested source's items must NOT install under --json --learn: {}",
        r.stdout
    );
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "the nested source's items must NOT install under --json --learn: {}",
        r.stdout
    );
    // The nested source is still registered; only its install pass is skipped.
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("nested"),
        "the nested source must still be registered: {}",
        sources.stdout
    );
}

#[test]
fn remeld_learn_json_folds_into_one_already_melded_document() {
    // spec: CLI-236 CLI-217 -- the re-meld `--json --learn` arm
    // (commands.rs:4131-4146) is a second, independent implementation of the
    // CLI-156 folding that had no test at all before this one. If it called
    // the whole-set install helper instead of the matching one, a re-meld
    // under `--json --learn x --yes` would install (and report) the entire
    // source undetected.
    let sb = Sandbox::new("lib");
    let first = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(first.success, "register-only meld failed: {}", first.stderr);
    assert!(!sb.claude_home.join("skills/review").exists());

    let r = sb.mind(&[
        "--json",
        "meld",
        &sb.source_spec(),
        "--learn",
        "review",
        "--yes",
    ]);
    assert!(
        r.success,
        "re-meld --learn --json failed: {} {}",
        r.stdout, r.stderr
    );
    // `serde_json::from_str` on the whole of stdout only succeeds if it is
    // exactly one JSON value: a second document concatenated after the first
    // would surface as a trailing-characters parse error here (CLI-153).
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    assert_eq!(
        doc["outcome"].as_str(),
        Some("already-melded"),
        "a re-meld's JSON result must report the CLI-12 already-melded outcome: {}",
        r.stdout
    );
    let installed = doc["installed"].as_array().expect("installed array");
    assert_eq!(
        installed
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["skill:review"],
        "only the matched item must be installed on a re-meld, not the whole \
         source: {}",
        r.stdout
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the matched item must actually install: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "an unmatched item must NOT be installed on a re-meld: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("rules/style.md").exists(),
        "an unmatched item must NOT be installed on a re-meld: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_pattern_does_not_reach_a_nested_sources_item() {
    // spec: CLI-236 -- the other half of the Scope paragraph that
    // `meld_learn_notes_that_it_ignores_recursive` leaves untested: a pattern
    // that NAMES an item belonging to a nested source (not the source being
    // melded) must be `LearnPatternNoMatch`, exactly as if it matched
    // nothing at all, rather than reaching across into the nested source and
    // installing its item. Nothing here fails if `learn_pattern_targets`'
    // `it.source == source_name` filter were loosened to
    // `resolve::source_matches` (which permits a component-suffix match, the
    // ergonomic behavior `--source` selectors elsewhere rely on): to prove
    // that, the nested item is reached through an item-link source whose
    // registered identity is deliberately deep enough that its trailing path
    // components, at a `/` boundary, spell out the CURATOR's own identity --
    // exactly the shape `source_matches` would treat as "the same source,
    // named by a shorter suffix". The curator's own repo directory is named
    // "review" (a skill's bare name is always its directory's basename,
    // ignoring frontmatter `name:`, per `catalog::make_item`), so the nested
    // item, whose item-link path is forced to end in that same "review"
    // component to spell out the curator's identity, ends up with the SAME
    // bare name as the `--learn` pattern below -- the one thing a reach-across
    // needs to actually happen.
    let registry = Sandbox::bare("review");
    let curator_identity = registry.identity();

    // The nested repo's one skill lives at a path that embeds the curator's
    // own identity as a trailing component sequence, so a source-matches-style
    // suffix check (but not exact equality) would treat the nested source as
    // "the same source" as the curator.
    let linkrepo = Sandbox::bare("linkrepo");
    let item_path = format!("nested-item/{curator_identity}");
    linkrepo.write_and_commit(
        &format!("{item_path}/SKILL.md"),
        "---\ndescription: A nested item; must not be reachable from the \
         curator's --learn\n---\n# review\n",
    );
    let link_url = linkrepo.link(&format!("tree/main/{item_path}"));

    registry.write_and_commit(
        "mind.toml",
        &format!("[discover]\nsources = [{{ source = {link_url:?}, install = true }}]\n"),
    );

    let r = registry.mind(&[
        "meld",
        &registry.source_spec(),
        "--learn",
        "review",
        "--yes",
    ]);
    assert!(
        !r.success,
        "a pattern naming only a NESTED source's item must fail, not reach \
         across and install it: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("matches no item in source") && r.stderr.contains("review"),
        "the failure must be the source-scoped LearnPatternNoMatch, not some \
         other error: {}",
        r.stderr
    );
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "the nested source's item must NOT be installed by a pattern that \
         only names the curator being melded: {:?}",
        registry.claude_home
    );
    // The source stays melded (CLI-236), and the nested source is still
    // registered -- only reachable through it, not through the curator.
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains(&curator_identity) && sources.stdout.contains("linkrepo"),
        "the curator and the nested source both stay melded after the failed \
         install pass: {}",
        sources.stdout
    );
}

#[test]
fn meld_learn_json_folds_installed_keys_into_the_one_object() {
    // spec: CLI-236 CLI-156 -- under --json with --yes the installed keys ride
    // in the single meld object.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&[
        "--json",
        "meld",
        &sb.source_spec(),
        "--learn",
        "review",
        "--yes",
    ]);
    assert!(r.success, "meld --learn --json failed: {}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    let installed = doc["installed"].as_array().expect("installed array");
    assert_eq!(
        installed
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["skill:review"],
        "only the matched item is reported installed: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_json_pending_counts_the_union_not_the_sum() {
    // spec: CLI-236 -- without --yes nothing installs between patterns, so
    // summing per-pattern counts would count an item two patterns both match,
    // or a shared dependency, more than once.
    let sb = Sandbox::new("lib");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\nrequires: agent:dev\n---\n# review\n",
    );
    let r = sb.mind(&[
        "--json",
        "meld",
        &sb.source_spec(),
        "--learn",
        "review",
        "--learn",
        "skill:*",
        "--learn",
        "agent:dev",
    ]);
    assert!(r.success, "meld --learn --json failed: {}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    // Three patterns, but only two distinct items: skill:review and the
    // agent:dev it requires (matched directly by the third pattern too).
    assert_eq!(
        doc["pending_items"].as_u64(),
        Some(2),
        "pending must be the union of the closures: {}",
        r.stdout
    );
}

#[test]
fn meld_learn_off_a_tty_names_a_command_that_runs_verbatim() {
    // spec: CLI-236 CLI-225 -- the non-TTY branch's fallback is a pasteable
    // `mind learn <ref>` composed from `LearnTarget::display`. Asserting it
    // merely CONTAINS "mind learn" would pass on a ref that names the wrong
    // item, or on one `learn` cannot parse at all, so this runs the printed
    // command through the real binary and checks what it installs.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(!sb.claude_home.join("skills/review").exists());

    let command = backticked(&r.stdout);
    assert_eq!(
        command,
        format!("mind learn '{}#skill:review'", sb.identity()),
        "the fallback must name the exact, source-qualified, kind-qualified \
         identity of the match: {}",
        r.stdout
    );
    let run = run_printed(&sb, &command);
    assert!(
        run.success,
        "the printed fallback `{command}` must run: {} {}",
        run.stdout, run.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the printed fallback must install the item it named: {}",
        run.stdout
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "and only that item: {}",
        run.stdout
    );
}

#[test]
fn meld_learn_off_a_tty_names_the_literal_item_when_its_name_carries_glob_syntax() {
    // spec: CLI-236 -- `is_safe_item_name` permits `[`, and the `mind learn`
    // fallback is read back by `learn`, which treats `*`/`?`/`[` in the name
    // half as glob syntax (CLI-31). `LearnTarget::display` glob-escapes the key
    // for that reason; unescaped, the fallback printed for `pdf[x]` installs
    // `pdfx` instead. This repo ships both, so the substitution is observable.
    let sb = Sandbox::bare("lib");
    sb.write_and_commit(
        "skills/pdf[x]/SKILL.md",
        "---\ndescription: The literal one\n---\n# pdf[x]\n",
    );
    sb.write_and_commit(
        "skills/pdfx/SKILL.md",
        "---\ndescription: The one a glob would select\n---\n# pdfx\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "pdf[[]x[]]"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    let command = backticked(&r.stdout);
    assert_eq!(
        command,
        format!("mind learn '{}#skill:pdf[[]x[]]'", sb.identity()),
        "the fallback must carry the glob-escaped key: {}",
        r.stdout
    );
    let run = run_printed(&sb, &command);
    assert!(
        run.success,
        "the printed fallback `{command}` must run: {} {}",
        run.stdout, run.stderr
    );
    assert!(
        sb.claude_home.join("skills/pdf[x]").exists(),
        "the fallback must install the literally named skill: {}",
        run.stdout
    );
    assert!(
        !sb.claude_home.join("skills/pdfx").exists(),
        "and not the one a glob reading selects: {}",
        run.stdout
    );
}

#[test]
fn meld_learn_off_a_tty_names_a_command_that_runs_verbatim_for_several_matches() {
    // spec: CLI-236 CLI-225 -- the same fallback when the batch has MORE THAN
    // ONE fresh match. `install_source_items_matching` joins every target into
    // a single `mind learn <a> <b> ...`, but `learn` takes exactly one
    // positional `<ITEM>` (cli.rs `Learn { item: String }`), so the joined form
    // is rejected by clap with "unexpected argument" the moment a second match
    // exists. CLI-236 says the non-TTY branch "prints how to install later";
    // a command that cannot run does not. The same join is used by the
    // interactive-decline branch's "skipped; run `mind learn ...`" line, so
    // both branches carry it. Fixing it means emitting one `mind learn` per
    // target (or a `&&`-joined sequence), not relaxing this test.
    let sb = Sandbox::new("lib");
    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "*"]);
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("3 item(s) match --learn"),
        "the fixture must produce a multi-item batch: {}",
        r.stdout
    );

    let command = backticked(&r.stdout);
    let run = run_printed(&sb, &command);
    assert!(
        run.success,
        "the printed fallback `{command}` must run: {} {}",
        run.stdout, run.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("agents/dev.md").exists()
            && sb.claude_home.join("rules/style.md").exists(),
        "the printed fallback must install every item it named: {} {}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn meld_learn_declined_at_the_prompt_installs_nothing_and_names_a_working_command() {
    // spec: CLI-236 -- the interactive half of the CLI-23 gate `--learn`
    // reuses. On a TTY the whole batch is previewed once and prompted once; a
    // decline installs nothing and prints the same pasteable fallback. Nothing
    // covered this branch: every other integration test is non-TTY, which takes
    // the note branch above instead. `MIND_TTY=1` (HOOK-109) forces the TTY
    // reading, and a `/dev/null` stdin reaches `read_confirm`'s EOF-is-No.
    let sb = Sandbox::new("lib");
    let r = sb.mind_env(
        &["meld", &sb.source_spec(), "--learn", "review"],
        &[("MIND_TTY", "1")],
        None,
    );
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("would learn 1 item(s)"),
        "the batch must be previewed before the prompt: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("install these 1 item(s) now?"),
        "the batch must be prompted for once: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skipped; run"),
        "a decline must say so: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "a decline must install nothing: {}",
        r.stdout
    );

    let command = backticked(
        r.stdout
            .split("skipped; run")
            .nth(1)
            .expect("the skip line"),
    );
    assert_eq!(
        command,
        format!("mind learn '{}#skill:review'", sb.identity()),
        "the decline fallback must name the exact identity: {}",
        r.stdout
    );
    let run = run_printed(&sb, &command);
    assert!(
        run.success && sb.claude_home.join("skills/review").exists(),
        "the decline fallback `{command}` must install what it named: {} {}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn meld_learn_accepted_at_the_prompt_installs_the_whole_batch_once() {
    // spec: CLI-236 -- the accept side of the same gate: ONE prompt for the
    // whole batch (not one per pattern), and the closure of each match comes
    // with it (DEP-30).
    let sb = Sandbox::new("lib");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\nrequires: agent:dev\n---\n# review\n",
    );
    let r = sb.mind_env(
        &[
            "meld",
            &sb.source_spec(),
            "--learn",
            "review",
            "--learn",
            "rule:style",
        ],
        &[("MIND_TTY", "1")],
        Some("y\n"),
    );
    assert!(r.success, "meld --learn failed: {} {}", r.stdout, r.stderr);
    assert_eq!(
        r.stdout.matches("install these").count(),
        1,
        "one prompt for the whole batch, not one per pattern: {}",
        r.stdout
    );
    assert!(
        sb.claude_home.join("skills/review").exists()
            && sb.claude_home.join("rules/style.md").exists()
            && sb.claude_home.join("agents/dev.md").exists(),
        "both matches and the closure of the first must install: {}",
        r.stdout
    );
}

#[test]
fn remeld_learn_installs_the_named_subset() {
    // spec: CLI-236 -- a re-meld (CLI-12) honors `--learn` too.
    let sb = Sandbox::new("lib");
    let first = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(first.success, "register-only meld failed: {}", first.stderr);
    assert!(!sb.claude_home.join("skills/review").exists());

    let r = sb.mind(&["meld", &sb.source_spec(), "--learn", "review", "--yes"]);
    assert!(
        r.success,
        "re-meld --learn failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the matched item must install on a re-meld: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "an unmatched item must NOT be installed: {}",
        r.stdout
    );
}

// ----- DSC-97: a `[[items]] link` must stay inside a kind directory -----

#[test]
fn link_outside_kind_directory_is_refused_end_to_end() {
    // spec: DSC-97 -- a `[[items]] link` pointed at the agent-home root (here
    // mimicking a Claude harness settings file) must be refused at meld,
    // before install ever runs, and no symlink must ever land at that path.
    let source = Sandbox::bare("hostile");
    write_file(
        &source.source.join("rules/x.md"),
        "---\ndescription: looks innocent\n---\n\
         {\"hooks\":{\"PreToolUse\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo pwned\"}]}]}}\n",
    );
    write_file(
        &source.source.join("mind.toml"),
        "[[items]]\nkind = \"rule\"\nname = \"x\"\npath = \"rules/x.md\"\nlink = \"settings.json\"\n",
    );
    git_commit(&source.source);

    let r = source.mind(&["meld", &source.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "melding a source with a link outside a kind directory must fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("kind directory") || r.stderr.contains("settings.json"),
        "the refusal must explain the confinement rule: {}",
        r.stderr
    );

    // Above all: the dangerous symlink must never be created, at the claimed
    // agent-home path or anywhere else in the (empty, since the meld failed)
    // claude home.
    let planted = source.claude_home.join("settings.json");
    assert!(
        !planted.exists(),
        "a hostile link target must never be planted as a symlink: {planted:?}"
    );
    assert!(
        !source.claude_home.exists()
            || std::fs::read_dir(&source.claude_home)
                .unwrap()
                .next()
                .is_none(),
        "the agent home must stay empty when meld is refused"
    );
}

#[test]
fn link_inside_kind_directory_still_installs() {
    // spec: DSC-97 -- a documented use case (TOOL-4: a tool surfaced under a
    // DIFFERENT kind directory than its own) is unaffected: confinement checks
    // against any of the four kind directories, not the item's own kind.
    let source = Sandbox::bare("cross-kind-link");
    write_file(&source.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write_file(
        &source.source.join("mind.toml"),
        "[[items]]\nkind = \"tool\"\nname = \"detect\"\npath = \"tools/detect\"\nlink = \"agents/detect\"\n",
    );
    git_commit(&source.source);

    let r = source.mind(&["meld", &source.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let link = source.claude_home.join("agents/detect");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "a link into a (different) kind directory must still be honored: {link:?}"
    );
}

#[test]
fn install_items_yes_flag_installs_subset_non_interactively() {
    // spec: DSC-62 — CLI-23: --yes installs the subset without prompting
    // (non-interactive / non-TTY run with --yes must install).
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\", \"agent:dev\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {}", r.stderr);

    // Both listed items are installed.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review must be installed with --yes"
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed with --yes"
    );
    // The unlisted item is not installed.
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "rule:style (not in install-items) must not be installed"
    );
}

#[test]
fn install_items_link_only_installs_nothing() {
    // spec: DSC-62 — CLI-23: --link-only skips install, even when install-items
    // is non-empty; only registration occurs.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(r.success, "meld --link-only failed: {}", r.stderr);

    // Nothing installed under link-only.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "--link-only must not install any items, even when install-items is set"
    );

    // The nested source is still registered.
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("nested"),
        "nested source must be registered under --link-only: {}",
        sources.stdout
    );
}

#[test]
fn install_items_empty_installs_nothing() {
    // spec: DSC-62 — install-items = [] is equivalent to install = false:
    // the nested source is registered, no items are offered for install.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // No items are installed.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "install-items = [] must not install any items"
    );
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "install-items = [] must not install any items"
    );
    // Items are still available.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:review"),
        "items must still be available after install-items = []: {}",
        probe.stdout
    );
}

// ----- DSC-62: recursive overrides install-items -----

#[test]
fn recursive_overrides_install_items_installs_all() {
    // spec: DSC-62 — meld --recursive is the superset: it installs every nested
    // source's items regardless of install-items, so install-items is effectively
    // ignored under --recursive.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--recursive", "--yes"]);
    assert!(r.success, "meld --recursive failed: {}", r.stderr);

    // All items installed, not just the listed subset.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review must be installed under --recursive"
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed under --recursive (overrides install-items)"
    );
    assert!(
        registry.claude_home.join("rules/style.md").exists(),
        "rule:style must be installed under --recursive (overrides install-items)"
    );
}

#[test]
fn install_items_non_tty_without_yes_installs_nothing_but_notes() {
    // spec: DSC-62 — CLI-23: in a non-interactive (non-TTY) run without --yes,
    // the subset is NOT installed; a note points at `mind learn`. The test
    // harness pipes stdin (Stdio::null), so the run is non-TTY.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // Plain meld: no --yes, no --link-only -> non-TTY note path.
    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Nothing is installed without --yes in a non-TTY run.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "non-TTY meld without --yes must NOT install the subset"
    );
    // A note points the user at how to install it.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("learn") && combined.contains("nested"),
        "a note should point at `mind learn` for the nested source: {combined}"
    );
}

#[test]
fn install_items_remeld_honors_subset() {
    // spec: DSC-62 — re-melding an already-registered super-source honors
    // install-items on the re-meld too (DSC-58/DSC-62 apply on fresh meld AND
    // re-meld). First meld --link-only (registers but installs nothing), then a
    // re-meld --yes must install exactly the subset.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // First meld: link-only, so nothing installs but the chain is registered.
    let first = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(first.success, "initial meld failed: {}", first.stderr);
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "nothing should be installed after --link-only meld"
    );

    // Re-meld with --yes: the super-source is already melded, so this goes
    // through the remeld path. It must still honor install-items.
    let second = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(
        second.success,
        "re-meld failed: {} {}",
        second.stdout, second.stderr
    );

    // The subset installs on re-meld.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "re-meld must honor install-items and install the subset"
    );
    // The unlisted items are still NOT installed on re-meld.
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "re-meld must not install items outside install-items"
    );
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "re-meld must not install items outside install-items"
    );
}

// ----- DSC-63: bad ref is a BadReference error at meld -----

#[test]
fn install_items_unknown_ref_errors_at_meld() {
    // spec: DSC-63 — a ref naming an item the nested source does NOT offer is a
    // BadReference error at meld, not a silent skip.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:nonexistent\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with an unknown install-items ref must fail"
    );
    // The error message must reference the bad ref.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("nonexistent"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_wrong_kind_ref_errors_at_meld() {
    // spec: DSC-63 — a ref of the wrong kind for an existing bare name is a
    // BadReference at meld. `review` exists only as a skill, so `agent:review`
    // names an item the nested source does not offer.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"agent:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "a wrong-kind ref (agent:review when review is a skill) must fail at meld"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("review"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_one_bad_ref_in_list_aborts_and_installs_nothing() {
    // spec: DSC-63 — a list with a valid and an invalid ref fails the whole meld
    // (BadReference, not a silent skip of just the bad one), and because the
    // error is raised before install, nothing from the subset is installed.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\", \"skill:ghost\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "a list containing one unknown ref must fail the meld"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("ghost"),
        "error must name the bad ref: {combined}"
    );
    // The valid ref must NOT have been installed: validation precedes install.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "a bad ref aborts before install; the valid item must not be installed"
    );
}

#[test]
fn install_items_prefixed_name_ref_is_rejected() {
    // spec: DSC-63 — refs are BARE kind:name in source truth. A ref written with
    // the prefix already applied (skill:pfx:review) does not name a real bare
    // item, so it is a BadReference even though the prefix is in effect. The
    // BadReference check must compare against the bare name, not reject a ref
    // merely because a prefix is set.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:pfx:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "a ref written with the prefix (skill:pfx:review) is not a bare name and must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("pfx:review"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_bare_ref_accepted_despite_prefix_in_effect() {
    // spec: DSC-63 — the converse of the rejection test: a BARE ref must be
    // accepted (not rejected) even when a prefix is in effect for the entry, and
    // it installs under the prefixed effective name.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // No --yes: must not error at meld (the bare ref is valid). Use --link-only
    // so the meld validates the ref but does not depend on install behavior.
    let r = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(
        r.success,
        "a bare ref must be accepted when a prefix is in effect: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn install_items_prefix_applied_at_install_time() {
    // spec: DSC-63 — refs in install-items are bare (source truth); the prefix
    // in effect for the entry (`as`, DSC-39) is applied at install time.
    // A ref of "skill:review" with `as = "pfx"` installs as "pfx:review".
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // The item installs under the prefixed name.
    assert!(
        registry.claude_home.join("skills/pfx:review").exists(),
        "prefixed name pfx:review must be installed: {:?}",
        registry.claude_home
    );
    // The bare name link must NOT exist.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "bare name review must not exist when prefix is in effect"
    );
}

// ----- DSC-64: install = true + non-empty install-items is an error -----

#[test]
fn install_true_and_install_items_is_toml_error() {
    // spec: DSC-64 — install = true together with a non-empty install-items on
    // the same entry is a MindToml error at meld (mutually exclusive).
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with install = true + non-empty install-items must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mutually exclusive"),
        "error must mention 'mutually exclusive': {combined}"
    );
}

#[test]
fn install_true_with_empty_install_items_is_not_an_error() {
    // spec: DSC-64 — install = true and install-items = [] is NOT an error;
    // the empty list overrides the boolean (both say "install nothing effectively").
    // This is allowed per the spec: the mutual-exclusion error is only for non-empty.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true, install-items = [] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with install = true + empty install-items must succeed: {}",
        r.stderr
    );
}

// ----- DSC-62: install-items governs over install = false (absent) -----

#[test]
fn install_items_governs_when_present_regardless_of_install_flag() {
    // spec: DSC-62 — when install-items is present it governs; when absent the
    // install boolean governs. Two entries: one with install-items (governs),
    // one with install = true but no install-items (boolean governs).
    let nested_a = Sandbox::new("nested-a"); // has install-items
    let nested_b = Sandbox::bare("nested-b"); // has install = true
    nested_b.write_and_commit(
        "skills/special/SKILL.md",
        "---\nname: special\ndescription: Special skill\n---\n# special\n",
    );

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [\n  {{ source = {:?}, install-items = [\"skill:review\"] }},\n  {{ source = {:?}, install = true }}\n]\n",
            nested_a.source_spec(),
            nested_b.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // nested-a: only skill:review installed (install-items governs).
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review (in install-items) must be installed"
    );
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev (not in install-items for nested-a) must not be installed"
    );

    // nested-b: all items installed (install = true, no install-items).
    assert!(
        registry.claude_home.join("skills/special").exists(),
        "skill:special from nested-b (install = true) must be installed"
    );
}

// ----- DSC-91: metadata size cap -----
//
// Mirrors the documented cap in src/error.rs (`METADATA_SIZE_LIMIT`, 8 MiB):
// this integration suite hardcodes the same number rather than importing the
// binary's internal constant (an integration test only has the compiled
// binary to drive, not the crate's internals). If the cap is ever tuned, this
// constant needs a matching update.
const METADATA_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

#[test]
fn dsc91_oversized_mind_toml_refused_at_meld() {
    // spec: DSC-91 — an oversized mind.toml is refused at meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-mindtoml");
    registry.write_sparse_and_commit("mind.toml", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(!r.success, "meld with an oversized mind.toml must fail");
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind.toml"),
        "error must name mind.toml: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_mind_toml_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized mind.toml melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-mindtoml");
    registry.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"a perfectly normal source\"\n",
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized mind.toml must succeed: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn dsc91_oversized_skill_frontmatter_refused_at_meld() {
    // spec: DSC-91 — an oversized SKILL.md is refused at meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-skill");
    registry.write_sparse_and_commit("skills/huge/SKILL.md", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with an oversized SKILL.md frontmatter must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("SKILL.md"),
        "error must name SKILL.md: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_skill_frontmatter_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized skill melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-skill");
    registry.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized SKILL.md must succeed: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn dsc91_oversized_plugin_manifest_refused_at_meld() {
    // spec: DSC-91 — an oversized .claude-plugin/plugin.json is refused at
    // meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-plugin");
    registry.write_sparse_and_commit(".claude-plugin/plugin.json", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(!r.success, "meld with an oversized plugin.json must fail");
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin.json"),
        "error must name plugin.json: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_plugin_manifest_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized plugin.json melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-plugin");
    registry.write_and_commit(".claude-plugin/plugin.json", "{\"name\":\"myplugin\"}\n");

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized plugin.json must succeed: {} {}",
        r.stdout, r.stderr
    );
}

// ----- DSC-95: BadReference / uninstall-warning field sanitization -----
//
// `MindError::BadReference`'s Display interpolates `item`, `referent`, and
// `in_source` verbatim (error.rs owns that print format, not install.rs), so
// install.rs must sanitize each field at its construction site before a
// hostile source's raw bytes ever reach it. And the uninstall confinement
// warnings print a manifest-recorded path directly, which for an item
// installed by a pre-DSC-96 binary can carry the same kind of payload. Each
// test below is a PAIR of assertions: the raw ESC/BEL byte is gone from the
// full process output, AND enough of the value survives sanitizing to remain
// actionable -- proving the value was cleaned, not silently dropped.

#[test]
fn learn_with_hostile_ns_token_fails_without_leaking_escape_bytes() {
    // spec: DSC-95 -- a `{{ns:name}}` token whose inner text carries an OSC-52
    // (clipboard-write) escape payload, naming a sibling that does not exist,
    // must fail `mind learn` with output carrying no raw ESC/BEL byte.
    // Otherwise an ordinary `mind learn` of a hostile repo could repaint the
    // terminal or write the clipboard through the resulting BadReference text.
    let source = Sandbox::bare("ns-hostile");
    source.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n\
         {{ns:\x1b]52;c;UEFZTE9BRA==\x07evil}}\n",
    );

    let meld = source.mind(&["meld", &source.source_spec(), "--link-only"]);
    assert!(
        meld.success,
        "meld --link-only must register the source (the hostile token is only \
         validated at install time): {} {}",
        meld.stdout, meld.stderr
    );

    let learn = source.mind(&["learn", "skill:review"]);
    assert!(
        !learn.success,
        "learning an item with an unresolvable hostile {{{{ns:}}}} token must fail"
    );
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        !combined.contains('\x1b') && !combined.contains('\u{07}'),
        "the BadReference error must not leak the raw ESC/BEL bytes of the \
         OSC payload: {combined:?}"
    );
    assert!(
        combined.contains("evil"),
        "the error should still name enough of the referent to be \
         actionable: {combined}"
    );
}

#[test]
fn forget_with_hostile_link_outside_agent_homes_warns_without_leaking_escape() {
    // spec: DSC-95 -- the uninstall confinement warning at a `links` entry
    // that escapes every configured agent home must route the recorded path
    // through `sanitize::display_path`, not print it raw.
    let sb = Sandbox::new("m9-link");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the real review install must exist before the manifest hand-edit"
    );

    // Hand-edit manifest.json: clone the real "review" entry under a distinct
    // key, with a hostile ANSI-carrying `links` path that both escapes every
    // configured agent home (a `..` component) and differs from the real
    // install's recorded link, so removing it cannot touch the real files.
    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null(), "the review entry must exist: {doc}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String("hostile-link".to_string());
    hostile["bare_name"] = serde_json::Value::String("hostile-link".to_string());
    hostile["store"] = serde_json::Value::String("store/skill/hostile-link".to_string());
    let hostile_link = "/does-not-exist-mind-test-root\x1b[31mRED\x1b[0m/../evil".to_string();
    hostile["links"] = serde_json::Value::Array(vec![serde_json::Value::String(hostile_link)]);
    doc["items"]["skill:hostile-link"] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let forget = sb.mind(&["forget", "skill:hostile-link"]);
    let combined = format!("{}{}", forget.stdout, forget.stderr);
    assert!(
        !combined.contains('\x1b'),
        "the uninstall confinement warning must not leak a raw ESC byte: {combined:?}"
    );
    assert!(
        combined.contains("outside all configured agent homes"),
        "the warning text must still appear: {combined}"
    );
    assert!(
        combined.contains("RED"),
        "the warning must still show enough of the sanitized path to be \
         actionable: {combined}"
    );

    // The real "review" install is untouched: it was a distinct manifest key
    // with a distinct recorded link.
    assert!(sb.claude_home.join("skills/review").exists());
}

#[test]
fn forget_with_hostile_store_outside_root_warns_without_leaking_escape() {
    // spec: DSC-95 -- the uninstall confinement warning at a `store` entry
    // that escapes the mind store root must route the recorded path through
    // `sanitize::display_path`, not print it raw.
    let sb = Sandbox::new("m9-store");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);

    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null(), "the review entry must exist: {doc}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String("hostile-store".to_string());
    hostile["bare_name"] = serde_json::Value::String("hostile-store".to_string());
    // A `..` component lexically escapes `~/.mind/store` entirely (LIFE-44's
    // `is_confined_under` rejects any `..` outright), and an ANSI escape is
    // embedded in the path text itself.
    hostile["store"] =
        serde_json::Value::String("../outside-mind-store-root\x1b[31mRED\x1b[0m".to_string());
    hostile["links"] = serde_json::Value::Array(vec![]);
    doc["items"]["skill:hostile-store"] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let forget = sb.mind(&["forget", "skill:hostile-store"]);
    let combined = format!("{}{}", forget.stdout, forget.stderr);
    assert!(
        !combined.contains('\x1b'),
        "the uninstall confinement warning must not leak a raw ESC byte: {combined:?}"
    );
    assert!(
        combined.contains("outside the mind store root"),
        "the warning text must still appear: {combined}"
    );
    assert!(
        combined.contains("RED"),
        "the warning must still show enough of the sanitized path to be \
         actionable: {combined}"
    );

    assert!(sb.claude_home.join("skills/review").exists());
}
