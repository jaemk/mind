//! `evolve` (spec/cli.md).

use super::*;

#[test]
fn evolve_check_with_explicit_version_reports_update_and_changes_nothing() {
    // spec: CLI-141
    // `evolve --check --version <X>` makes zero network calls (an explicit
    // --version bypasses the GitHub API). When X > the running version, the
    // command must succeed and report the update as available.
    let sb = Sandbox::new(); // empty sandbox; no sources or manifest needed
    let r = sb.mind(&["evolve", "--check", "--version", "9.9.9"]);
    assert!(
        r.success,
        "evolve --check --version 9.9.9 should succeed: {} {}",
        r.stdout, r.stderr
    );
    // The output must contain the target version and signal it is available.
    assert!(
        r.stdout.contains("9.9.9"),
        "expected target version 9.9.9 in output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("available"),
        "expected 'available' in output: {}",
        r.stdout
    );
    // Nothing on disk changed: no source or manifest files exist in the sandbox.
    assert!(
        !sb.mind_home.join("sources.json").exists(),
        "no sources.json should be written by evolve --check"
    );
    assert!(
        !sb.mind_home.join("manifest.json").exists(),
        "no manifest.json should be written by evolve --check"
    );
}

#[test]
fn evolve_check_at_current_version_reports_up_to_date() {
    // spec: CLI-141
    // When the explicit --version equals the running binary version, evolve
    // --check reports up to date and exits zero, with zero network calls.
    let sb = Sandbox::new();
    let current = env!("CARGO_PKG_VERSION");
    let r = sb.mind(&["evolve", "--check", "--version", current]);
    assert!(
        r.success,
        "evolve --check --version {current} should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("up to date"),
        "expected 'up to date' in output for version {current}: {}",
        r.stdout
    );
}

#[test]
fn evolve_refuses_a_homebrew_managed_binary_and_check_points_at_brew() {
    // spec: CLI-243 -- a binary resolved through a Homebrew Cellar is brew's to
    // replace: evolve refuses before any network call, and --check still reports
    // with a `brew upgrade` pointer. The binary runs through a <prefix>/bin
    // symlink, the way brew links it, so the check must canonicalize.
    let sb = Sandbox::new();
    let cellar_bin = sb.base.join("brew/Cellar/mind/0.0.0/bin");
    std::fs::create_dir_all(&cellar_bin).unwrap();
    let cellar_exe = cellar_bin.join("mind");
    std::fs::copy(env!("CARGO_BIN_EXE_mind"), &cellar_exe).unwrap();
    let prefix_bin = sb.base.join("brew/bin");
    std::fs::create_dir_all(&prefix_bin).unwrap();
    let linked = prefix_bin.join("mind");
    std::os::unix::fs::symlink(&cellar_exe, &linked).unwrap();

    let run = |args: &[&str]| {
        // A dead proxy, so a regression that reaches the network fails fast
        // instead of making a real request.
        // A sibling test that forks while `fs::copy` still holds the copy's
        // write fd keeps it open in the child until exec, so a spawn in that
        // window fails with ETXTBSY. Retry briefly instead of flaking.
        let mut attempts = 0;
        let out = loop {
            let res = Command::new(&linked)
                .args(args)
                .env("MIND_HOME", &sb.mind_home)
                .env("CLAUDE_HOME", &sb.claude_home)
                .env("HTTPS_PROXY", "http://127.0.0.1:9")
                .env("https_proxy", "http://127.0.0.1:9")
                .stdin(Stdio::null())
                .output();
            match res {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => break other.expect("run linked mind"),
            }
        };
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    };

    let r = run(&["evolve", "--yes", "--to", "9.9.9"]);
    assert!(!r.success, "evolve must refuse: {} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("managed by Homebrew") && r.stderr.contains("brew upgrade mind"),
        "refusal must name Homebrew and the remedy: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("Cellar"),
        "refusal must name the resolved Cellar path: {}",
        r.stderr
    );

    let r = run(&["--json", "evolve", "--yes", "--to", "9.9.9"]);
    assert!(!r.success);
    assert!(
        r.stdout.contains("homebrew-managed") || r.stderr.contains("homebrew-managed"),
        "--json refusal must carry the homebrew-managed kind: {} {}",
        r.stdout,
        r.stderr
    );

    let r = run(&["evolve", "--check", "--to", "9.9.9"]);
    assert!(
        r.success,
        "--check must still report: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("brew upgrade mind"),
        "--check must point at brew upgrade: {}",
        r.stdout
    );

    // The same binary outside a Cellar is not refused: --check has no note.
    let r = sb.mind(&["evolve", "--check", "--to", "9.9.9"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("Homebrew"), "{}", r.stdout);
}
