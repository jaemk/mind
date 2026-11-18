//! End-to-end tests that drive the real `mind` binary against a hermetic,
//! network-free fixture (a local git repo melded via a filesystem path).
//!
//! Every manual smoke-test ("probe") of the CLI lives here as an assertion so
//! the behavior can be re-run and audited. See CLAUDE.md: manual checks must be
//! encoded as tests unless that is genuinely impossible.
//!
//! Tests are grouped by spec topic in the modules below; this file holds the
//! shared `Sandbox` fixture.

mod absorb;
mod config;
mod curate;
mod deps;
mod discovery;
mod dump;
mod evolve;
mod forget;
mod hooks;
mod init_source;
mod introspect;
mod learn;
mod lifecycle;
mod marketplace;
mod meld;
mod namespace;
mod output;
mod policy;
mod probe;
mod recall;
mod review;
mod storage;
mod sync;
mod tools;
mod unmanaged;
mod unmeld;
mod upgrade;
mod workflows;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../common/mod.rs"]
mod common;
use common::{git, write};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway environment: a source git repo plus isolated MIND_HOME/CLAUDE_HOME.
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
    /// Build a fixture source repo with one skill, one agent, and one rule.
    /// A source repo named `agents` carrying the standard fixture items.
    fn new() -> Sandbox {
        Sandbox::build("agents", true)
    }

    /// A source repo with a given name and the standard fixture items.
    fn named(name: &str) -> Sandbox {
        Sandbox::build(name, true)
    }

    /// A source repo with a given name and no items (e.g. a pure registry).
    fn bare(name: &str) -> Sandbox {
        Sandbox::build(name, false)
    }

    /// A source repo populated from `examples/<name>` in the crate, committed.
    /// Lets a test drive a shipped example so it cannot rot.
    fn from_example(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name);
        copy_dir(&example, &source);
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    /// A source repo carrying the crate's real root `mind.toml` plus the
    /// `examples/hello` directory it points at, committed. Drives the
    /// landing-page command (`mind meld jaemk/mind`, then `mind learn
    /// hello-mind` in a non-TTY).
    fn from_root_mindfile() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("mind");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("home"),
            claude_home: base.join("claude"),
        };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&source).unwrap();
        // The real root mind.toml curates two REMOTE skill libraries via
        // [discover].sources; melding it would clone them over the network, which
        // the hermetic harness forbids. Substitute local stand-in repos for those
        // URLs so the meld runs fully offline while still exercising the
        // register-only curated chain alongside the repo's own hello-mind
        // convention discovery. The real file's discover block is validated
        // offline by a unit test in src/mindfile.rs.
        let nested_a = base.join("anthropics-skills");
        write(
            &nested_a.join("skills/astand/SKILL.md"),
            "---\nname: astand\ndescription: stand-in for a curated skill\n---\n# astand\n",
        );
        git(&nested_a, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&nested_a, &["config", "user.email", "t@t"]);
        git(&nested_a, &["config", "user.name", "t"]);
        git(&nested_a, &["add", "-A"]);
        git(&nested_a, &["commit", "-qm", "initial"]);
        let nested_b = base.join("awesome-claude-skills");
        write(
            &nested_b.join("README.md"),
            "# awesome (stand-in, no items)\n",
        );
        git(&nested_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&nested_b, &["config", "user.email", "t@t"]);
        git(&nested_b, &["config", "user.name", "t"]);
        git(&nested_b, &["add", "-A"]);
        git(&nested_b, &["commit", "-qm", "initial"]);

        let mindfile = std::fs::read_to_string(root.join("mind.toml")).unwrap();
        let mindfile = mindfile
            .replace(
                "https://github.com/anthropics/skills",
                nested_a.to_str().unwrap(),
            )
            .replace(
                "https://github.com/ComposioHQ/awesome-claude-skills",
                nested_b.to_str().unwrap(),
            );
        write(&source.join("mind.toml"), &mindfile);
        copy_dir(&root.join("examples/hello"), &source.join("examples/hello"));
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    fn build(name: &str, with_fixture: bool) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        if with_fixture {
            write(
                &source.join("skills/review/SKILL.md"),
                "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\n",
            );
            write(
                &source.join("agents/dev.md"),
                "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\n",
            );
            write(
                &source.join("rules/style.md"),
                "---\ndescription: ASCII only\n---\n# style rule\n",
            );
        } else {
            write(&source.join("README.md"), "# registry\n");
        }

        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    /// Run `mind <args>` against this sandbox.
    fn mind(&self, args: &[&str]) -> Run {
        self.run(args, None, &[])
    }

    fn mind_with_input(&self, args: &[&str], input: Option<&str>) -> Run {
        self.run(args, input, &[])
    }

    /// Run `mind` as if at a terminal (`MIND_TTY=1`, HOOK-109) with `input`
    /// as the scripted replies: prompts are shown only on a TTY (CLI-259).
    fn mind_tty(&self, args: &[&str], input: &str) -> Run {
        self.run(args, Some(input), &[("MIND_TTY", "1")])
    }

    /// Run `mind` with additional environment variables (e.g. MIND_AGENT_HOMES).
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.run(args, None, envs)
    }

    /// Run `mind` with the child's working directory set to `cwd` (for testing
    /// how relative paths are resolved).
    fn mind_cwd(&self, args: &[&str], cwd: &Path) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .current_dir(cwd)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn run(&self, args: &[&str], input: Option<&str>, envs: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn mind");
        if let Some(text) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().expect("wait mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Change the skill upstream and commit, so a `sync` + `upgrade` sees a delta.
    fn edit_source(&self) {
        write(
            &self.source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
        );
        git(&self.source, &["commit", "-aqm", "edit review"]);
    }

    /// Write a file under the source repo and commit it.
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    /// Remove a file from the source repo and commit it.
    fn remove_and_commit(&self, rel: &str) {
        std::fs::remove_file(self.source.join(rel)).unwrap();
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "remove"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The base dir name, which becomes the `owner` for this sandbox's local
    /// source (so the source identity is `<base_name>/<source dir name>`).
    fn base_name(&self) -> String {
        self.base
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Recursively copy `src` into `dst` (files and subdirectories).
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Assert no `review-*` scratch dir survives under `<mind_home>/.tmp` (the
/// remote-clone area). CLI-130: review changes nothing on disk.
fn assert_no_review_temp(mind_home: &Path) {
    let tdir = mind_home.join(".tmp");
    if !tdir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(&tdir).unwrap().flatten() {
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with("review-"),
            "leftover review temp dir: {:?}",
            entry.path()
        );
    }
}

/// Meld + learn the standard fixture; returns the ready sandbox.
fn melded() -> Sandbox {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {}", r.stderr);
    sb
}

// ---------------------------------------------------------------------------
// CLI-225: printed-remedy shell-quoting helpers, reproduced here (not
// imported) since an integration test is a separate crate from the binary
// and cannot reach `crate::error::shell_quote` (`pub(crate)`). Kept a
// byte-for-byte mirror of the production algorithm in `src/error.rs` and the
// `shell_split`/round-trip technique `tests/cli_hooks.rs` uses for the
// HOOK-106 family, applied here to the CLI-225 sites in `commands/`/
// `install.rs`.
// ---------------------------------------------------------------------------

/// Mirrors `crate::error::shell_quote`: POSIX single-quote, with an embedded
/// `'` escaped via the `'\''` idiom.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A minimal shell tokenizer understanding only what `mind`'s CLI-225 remedies
/// ever emit: whitespace-separated words and a `shell_quote`d single-quoted
/// segment (including the `'\''` embedded-quote idiom). See
/// `tests/cli_hooks.rs`'s `shell_split` for the original.
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

/// Build a git repo at `dir` holding one skill named `skill`.
fn nested_repo(dir: &Path, skill: &str) {
    write(
        &dir.join(format!("skills/{skill}/SKILL.md")),
        &format!("---\nname: {skill}\ndescription: {skill} skill\n---\n# {skill}\n"),
    );
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "initial"]);
}

/// Meld a default `agents` source plus a second `tools` source carrying a
/// uniquely-named skill, so a `--source` filter can be checked for exclusion.
/// Both sandboxes are returned so the caller keeps the linked source dirs alive
/// (a local source is linked by path, not copied, so its dir must survive).
fn melded_two_sources() -> (Sandbox, Sandbox) {
    let agents = melded();
    // A bare source (no standard fixture) carrying only a uniquely-named skill,
    // so its items never overlap the agents source.
    let tools = Sandbox::bare("tools");
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of second source failed"
    );
    (agents, tools)
}

/// Write a fake `git` script to `<dir>/bin/git` and return the bin dir path.
/// The fake git exits non-zero with an auth-failure message whenever it is called
/// with `clone` and the URL starts with `https://`. All other invocations are
/// delegated to the real git.
fn fake_git_bin_dir(dir: &Path) -> PathBuf {
    // Resolve the real git once at test time.
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  for a; do\n    case \"$a\" in\n      https://*)\n        echo \"fatal: Authentication failed for '$a'\" >&2\n        exit 128\n        ;;\n    esac\n  done\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

/// Build a PATH string with `extra_dir` prepended to the current PATH.
fn prepend_path(extra_dir: &Path) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    if current.is_empty() {
        extra_dir.display().to_string()
    } else {
        format!("{}:{}", extra_dir.display(), current)
    }
}

/// Place an unmanaged skill (a dir) and agent (a file) directly in the lobe.
fn seed_unmanaged(sb: &Sandbox) {
    write(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    write(
        &sb.claude_home.join("agents/custom.md"),
        "---\nname: custom\n---\n# custom\n",
    );
}

/// Build a sandbox repo that has a `stable` branch and a `v1.0` tag at the
/// initial commit, then advance `main` further. Returns (sandbox, sha_at_v1_0,
/// sha_at_main_tip).
fn make_pinnable_repo(name: &str) -> (Sandbox, String, String) {
    let sb = Sandbox::bare(name);

    // Write an initial file and commit it. This becomes the tagged commit.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v1\n---\n# dev v1\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "initial"]);

    // Read the sha of that initial commit.
    let sha_v1 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    // Tag it and create a `stable` branch pointing here.
    git(&sb.source, &["tag", "v1.0"]);
    git(&sb.source, &["branch", "stable"]);

    // Advance main with a second commit.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v2\n---\n# dev v2\n",
    );
    git(&sb.source, &["commit", "-aqm", "v2 commit"]);

    let sha_v2 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    (sb, sha_v1, sha_v2)
}

/// Read the `pin` field from a source's entry in sources.json.  Returns the
/// JSON object as a string so callers can assert on kind/value without pulling
/// in a serde dependency here.
fn read_source_pin_json(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Extract the `pin` object from the JSON.  The file is pretty-printed so
    // the pin block spans multiple lines; grab everything between `"pin": ` and
    // the next top-level `}` after it.
    let start = json.find("\"pin\":").expect("pin key in sources.json");
    // Find the matching `}` for the pin object.
    let after = &json[start..];
    let obj_start = after.find('{').expect("pin object open brace");
    let obj_str = &after[obj_start..];
    let mut depth = 0usize;
    let mut end = 0;
    for (i, c) in obj_str.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    obj_str[..end].to_string()
}

/// Read the recorded commit for the first source in sources.json.
fn read_source_commit(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Extract "commit": "sha" from the JSON.
    let key = "\"commit\": \"";
    let start = json.find(key).expect("commit key") + key.len();
    let end = json[start..].find('"').expect("closing quote") + start;
    json[start..end].to_string()
}

/// The on-disk clone dir for the sandbox's local source:
/// `<mind_home>/sources/local/<base_name>/<repo>`.
fn clone_dir_of(sb: &Sandbox, repo: &str) -> PathBuf {
    sb.mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join(repo)
}

/// Read the `roots` field from the first source in sources.json as a JSON
/// string (for assertions without pulling in a serde dependency in tests).
fn read_source_roots_json(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Look for "roots": [ ... ]; return the whole array value.
    if let Some(start) = json.find("\"roots\":") {
        let after = &json[start + "\"roots\":".len()..];
        // Find the opening bracket.
        if let Some(arr_start) = after.find('[') {
            let arr = &after[arr_start..];
            let mut depth = 0usize;
            let mut end = 0;
            for (i, c) in arr.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return arr[..end].to_string();
        }
    }
    // No roots field: return "null" to signal absence.
    "null".to_string()
}

/// Write a policy TOML to the sandbox base and return its absolute path string,
/// for use as the `MIND_POLICY_FILE` env value.
fn write_policy(sb: &Sandbox, body: &str) -> String {
    let path = sb.base.join("policy.toml");
    write(&path, body);
    path.to_string_lossy().into_owned()
}

/// Count the melded sources by reading sources.json (0 when the file is absent).
fn source_count(sb: &Sandbox) -> usize {
    let path = sb.mind_home.join("sources.json");
    let Ok(json) = std::fs::read_to_string(&path) else {
        return 0;
    };
    json.matches("\"url\"").count()
}

/// A sandbox whose source declares an install hook command in mind.toml.
/// `[source]` with only `install =` is NOT authoritative, so the three
/// convention items are still discovered.
fn sandbox_with_declared_hook(name: &str, cmd: &str) -> Sandbox {
    let sb = Sandbox::named(name);
    sb.write_and_commit("mind.toml", &format!("[source]\ninstall = \"{cmd}\"\n"));
    sb
}

/// Parse `stdout` as a single JSON value, failing loudly with the raw text.
fn parse_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON ({e}): {stdout:?}"))
}

/// True if `s` carries any ANSI escape (ESC, 0x1b).
fn has_ansi_escape(s: &str) -> bool {
    s.contains('\u{1b}')
}

/// Read the whole `sources.json` for a sandbox.
fn read_sources_json(sb: &Sandbox) -> String {
    std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json")
}

/// A bare source whose skill directories sit directly at the repo root (no
/// `skills/` container), plus a conventional `agents/` dir. With `mindfile` set,
/// also writes a `mind.toml` at the root. No commit beyond the initial bare one
/// unless committed by the caller.
fn make_flat_source(name: &str, mindfile: Option<&str>) -> Sandbox {
    let sb = Sandbox::bare(name);
    write(
        &sb.source.join("alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Alpha flat skill\n---\n# alpha\n",
    );
    write(
        &sb.source.join("beta/SKILL.md"),
        "---\nname: beta\ndescription: Beta flat skill\n---\n# beta\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: A dev agent\n---\n# dev\n",
    );
    if let Some(toml) = mindfile {
        write(&sb.source.join("mind.toml"), toml);
    }
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "flat layout"]);
    sb
}

/// The manifest's installed keys, sorted.
fn installed_keys(sb: &Sandbox) -> Vec<String> {
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let mut keys: Vec<String> = m["items"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}
