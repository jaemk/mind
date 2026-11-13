//! Progress spinner on stderr (spec/progress.md). The tests force it on with
//! `MIND_PROGRESS=always`, which draws every step immediately even off a
//! terminal, and read what reached each stream.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Return to column 0 and erase the line: the spinner's draw and clear prefix.
const CLEAR: &str = "\r\x1b[2K";

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

struct Sandbox {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-progress-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("mind")).unwrap();
        std::fs::create_dir_all(base.join("claude")).unwrap();
        Sandbox {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    fn cmd(&self, progress: Option<&str>, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env("LC_ALL", "C.UTF-8")
            .env("COLUMNS", "400")
            .env_remove("MIND_PROGRESS")
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_POLICY_FILE")
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_MAX_PARALLEL_FETCHES")
            .stdin(Stdio::null());
        if let Some(p) = progress {
            cmd.env("MIND_PROGRESS", p);
        }
        cmd
    }

    fn run(&self, progress: Option<&str>, args: &[&str]) -> Run {
        let out = self
            .cmd(progress, args)
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

    /// Run with stdout and stderr into one file, the way a terminal sees them
    /// interleaved.
    fn run_combined(&self, progress: Option<&str>, args: &[&str]) -> (String, bool) {
        let path = self.base.join("combined.out");
        let file = std::fs::File::create(&path).unwrap();
        let status = self
            .cmd(progress, args)
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .status()
            .expect("run mind");
        (std::fs::read_to_string(&path).unwrap(), status.success())
    }

    /// A git repo with one skill, plus `mind_toml` when given.
    fn repo(&self, name: &str, mind_toml: Option<&str>) -> String {
        let dir = self.base.join(name);
        write(
            &dir.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: Demo skill\n---\nbody\n",
        );
        if let Some(toml) = mind_toml {
            write(&dir.join("mind.toml"), toml);
        }
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        dir.to_string_lossy().into_owned()
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
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        // Fixed dates, so two fixtures built in different seconds get the
        // same commit and the stdout comparison holds.
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Whether `s` carries any spinner output.
fn has_spinner(s: &str) -> bool {
    s.contains(CLEAR)
}

/// The spinner draws in `s`: each segment after a CLEAR that starts with a
/// frame glyph. The ticker animates in the background, so any frame may lead.
fn draws(s: &str) -> Vec<&str> {
    s.split(CLEAR)
        .filter(|seg| seg.starts_with(FRAMES))
        .collect()
}

/// The labels drawn in `s`.
fn labels(s: &str) -> Vec<&str> {
    draws(s)
        .into_iter()
        .filter_map(|d| d.split_once(' ').map(|(_, label)| label))
        .collect()
}

// spec: PRG-1 PRG-4
#[test]
fn always_draws_labeled_steps_on_stderr_and_leaves_stdout_alone() {
    let sb = Sandbox::new();
    let repo = sb.repo("src", None);
    let r = sb.run(Some("always"), &["meld", &repo, "--pin", "HEAD", "--yes"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let shown = labels(&r.stderr);
    assert!(
        shown.contains(&format!("cloning {repo}").as_str()),
        "the clone shows a labeled step: {shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|l| l.starts_with("installing ") && l.contains("demo")),
        "the install shows a labeled step: {shown:?}"
    );
    let tail = r.stderr.rsplit(CLEAR).next().unwrap();
    assert!(
        !tail.starts_with(FRAMES),
        "the last step is cleared: {:?}",
        r.stderr
    );
    assert!(
        !has_spinner(&r.stdout) && !r.stdout.contains(FRAMES),
        "stdout carries no spinner: {:?}",
        r.stdout
    );

    // The same meld with progress off prints the same stdout.
    let off = Sandbox::new();
    let repo_off = off.repo("src", None);
    let r_off = off.run(
        Some("never"),
        &["meld", &repo_off, "--pin", "HEAD", "--yes"],
    );
    assert!(r_off.success, "{}{}", r_off.stdout, r_off.stderr);
    let norm = |s: &str, sb: &Sandbox| {
        let name = sb.base.file_name().unwrap().to_string_lossy().into_owned();
        s.replace(&*sb.base.to_string_lossy(), "<base>")
            .replace(&name, "<base>")
    };
    assert_eq!(norm(&r.stdout, &sb), norm(&r_off.stdout, &off));
}

// spec: PRG-2
#[test]
fn progress_is_off_by_flag_json_never_and_off_a_terminal() {
    let sb = Sandbox::new();
    let repo = sb.repo("src", None);
    let cases: [(Option<&str>, &[&str]); 4] = [
        (Some("always"), &["--no-progress"]),
        (Some("always"), &["--json"]),
        (Some("never"), &[]),
        // auto: stderr is a pipe here, so nothing draws.
        (None, &[]),
    ];
    for (i, (setting, extra)) in cases.iter().enumerate() {
        let home = Sandbox::new();
        let mut args = vec!["meld", repo.as_str(), "--pin", "HEAD", "--yes"];
        args.extend_from_slice(extra);
        let r = home.run(*setting, &args);
        assert!(r.success, "case {i}: {}{}", r.stdout, r.stderr);
        assert!(
            !has_spinner(&r.stderr) && !has_spinner(&r.stdout),
            "case {i} ({setting:?} {extra:?}) drew a spinner: {:?}",
            r.stderr
        );
    }
}

// spec: PRG-4
#[test]
fn parallel_sync_counts_finished_sources() {
    let sb = Sandbox::new();
    for name in ["one", "two"] {
        let repo = sb.repo(name, None);
        let r = sb.run(
            Some("never"),
            &["meld", &repo, "--pin", "HEAD", "--register-only"],
        );
        assert!(r.success, "{}{}", r.stdout, r.stderr);
    }
    let r = sb.run(Some("always"), &["sync"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let shown = labels(&r.stderr);
    assert!(
        shown.contains(&"syncing sources (0/2)") && shown.contains(&"syncing sources (2/2)"),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|l| l.starts_with("fetching ")),
        "a worker's git call does not replace the counted label: {shown:?}"
    );
}

/// The first `git` on `PATH`.
#[cfg(unix)]
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH")
}

// spec: PRG-10
#[cfg(unix)]
#[test]
fn ctrl_c_erases_a_drawn_spinner_line() {
    erases_the_line_on(libc::SIGINT);
}

// spec: PRG-10
#[cfg(unix)]
#[test]
fn sigterm_erases_a_drawn_spinner_line() {
    erases_the_line_on(libc::SIGTERM);
}

/// Send `sig` to a `mind` whose clone step is drawn, and check it dies of
/// the signal with the line erased.
#[cfg(unix)]
fn erases_the_line_on(sig: libc::c_int) {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;

    let sb = Sandbox::new();
    let repo = sb.repo("src", None);
    // A git whose clone hangs, so the clone step is drawn when the signal
    // lands; every other subcommand is the real git.
    let bin = sb.base.join("bin");
    let stub = bin.join("git");
    write(
        &stub,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = clone ]; then exec sleep 5; fi\nexec '{}' \"$@\"\n",
            real_git().display()
        ),
    );
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();

    let err_path = sb.base.join("stderr.out");
    let mut child = sb
        .cmd(Some("always"), &["meld", &repo, "--pin", "HEAD", "--yes"])
        .env("PATH", path)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .expect("run mind");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !labels(&std::fs::read_to_string(&err_path).unwrap())
        .iter()
        .any(|l| l.starts_with("cloning "))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the clone step never drew"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // SAFETY: signals the child this test spawned.
    unsafe {
        libc::kill(child.id() as libc::pid_t, sig);
    }
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(sig), "mind dies of the signal");
    let err = std::fs::read_to_string(&err_path).unwrap();
    assert!(err.ends_with(CLEAR), "the spinner line is erased: {err:?}");
}

// spec: PRG-4
#[test]
fn a_command_blocked_on_the_lock_says_it_is_waiting() {
    const WAITING: &str = "waiting for another mind command to finish";
    let sb = Sandbox::new();
    let mut held = fd_lock::RwLock::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(sb.mind_home.join(".lock"))
            .unwrap(),
    );
    let guard = held.write().unwrap();

    let err_path = sb.base.join("stderr.out");
    let mut child = sb
        .cmd(Some("always"), &["recall"])
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .expect("run mind");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !labels(&std::fs::read_to_string(&err_path).unwrap()).contains(&WAITING) {
        assert!(
            std::time::Instant::now() < deadline,
            "the wait step never drew"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(guard);
    assert!(child.wait().unwrap().success());

    // Uncontended, the same command never shows it.
    let r = sb.run(Some("always"), &["recall"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(!labels(&r.stderr).contains(&WAITING), "{:?}", r.stderr);
}

// spec: PRG-4 STO-85
#[test]
fn a_blocked_command_names_the_recorded_holder() {
    let sb = Sandbox::new();
    let mut held = fd_lock::RwLock::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(sb.mind_home.join(".lock"))
            .unwrap(),
    );
    let guard = held.write().unwrap();
    // This test process stands in for the holder, so the pid is live.
    let pid = std::process::id();
    write(
        &sb.mind_home.join(".lock.holder"),
        &format!(r#"{{"pid":{pid},"command":"mind upgrade --yes","started":0}}"#),
    );

    let err_path = sb.base.join("stderr.out");
    let mut child = sb
        .cmd(Some("always"), &["recall"])
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .expect("run mind");
    let want = format!(
        "waiting for another mind command to finish: mind upgrade --yes (pid {pid}, running "
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !labels(&std::fs::read_to_string(&err_path).unwrap())
        .iter()
        .any(|l| l.starts_with(&want))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the wait step never named the holder: {:?}",
            std::fs::read_to_string(&err_path).unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(guard);
    assert!(child.wait().unwrap().success());
}

// spec: STO-84
#[test]
fn a_mutating_command_leaves_no_holder_record_behind() {
    let sb = Sandbox::new();
    let repo = sb.repo("src", None);
    let r = sb.run(Some("never"), &["meld", &repo, "--pin", "HEAD", "--yes"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(sb.mind_home.join(".lock").exists());
    assert!(!sb.mind_home.join(".lock.holder").exists());
}

// spec: PRG-5
#[test]
fn nothing_else_lands_on_a_drawn_spinner_line() {
    let sb = Sandbox::new();
    let repo = sb.repo(
        "src",
        Some("[[hooks]]\nname = \"setup\"\nrun = \"echo hook-ran\"\nevent = \"install\"\n"),
    );
    let (out, ok) = sb.run_combined(
        Some("always"),
        &["meld", &repo, "--pin", "HEAD", "--yes", "--accept-hooks"],
    );
    assert!(ok, "{out}");
    assert!(out.contains("hook-ran"), "the hook ran: {out:?}");
    assert!(has_spinner(&out), "progress drew: {out:?}");
    // Every draw is `CLEAR <frame> <label>` and is followed by the next CLEAR
    // (a redraw or the clear), never by other output: a draw segment holds
    // no newline.
    for seg in draws(&out) {
        assert!(
            !seg.contains('\n'),
            "output was written onto a spinner line: {seg:?}\nfull: {out:?}"
        );
    }
}
