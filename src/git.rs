//! Thin wrapper around the `git` CLI, surfacing failures as [`MindError::Git`].

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{MindError, Result};
use crate::source::Pin;

/// When set, every `git` child runs non-interactively: it never prompts on the
/// controlling terminal for credentials, an SSH passphrase, or a host-key
/// confirmation. The TUI turns this on while it owns the terminal so an
/// auth-required remote fails fast with an error instead of hanging the UI on a
/// hidden prompt; the suspended interactive meld (term::with_suspended) turns it
/// back off so a real passphrase/host-key prompt works on the normal terminal.
static NONINTERACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the process-wide non-interactive git mode (see [`NONINTERACTIVE`]).
pub fn set_noninteractive(on: bool) {
    NONINTERACTIVE.store(on, Ordering::Relaxed);
}

/// The env pairs that make a `git` child non-interactive. `GIT_TERMINAL_PROMPT=0`
/// stops git's own credential prompts; wrapping the ssh command in `BatchMode=yes`
/// stops ssh's passphrase and host-key prompts (`base_ssh` preserves a user's
/// custom ssh invocation). A short `ConnectTimeout` avoids a long network hang.
// spec: TUI-45
fn noninteractive_env_pairs(base_ssh: &str) -> [(&'static str, String); 2] {
    [
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        (
            "GIT_SSH_COMMAND",
            format!("{base_ssh} -o BatchMode=yes -o ConnectTimeout=10"),
        ),
    ]
}

/// Apply the non-interactive environment to a `git` child when the mode is on.
fn apply_noninteractive_env(cmd: &mut Command) {
    if !NONINTERACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let base = std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh".to_string());
    for (k, v) in noninteractive_env_pairs(&base) {
        cmd.env(k, v);
    }
}

/// Run `git <args>` in `cwd`, returning trimmed stdout on success.
fn run(url: &str, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args);
    apply_noninteractive_env(&mut cmd);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(MindError::GitNotFound),
        Err(e) => {
            return Err(MindError::Git {
                url: url.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                status: None,
                stderr: e.to_string(),
            });
        }
    };

    if !output.status.success() {
        return Err(MindError::Git {
            url: url.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            status: Some(output.status),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clone `url` into `dest` at the point specified by `pin` (CLI-18).
///
/// - `DefaultBranch` — shallow clone of the remote default branch (original
///   behavior).
/// - `FollowBranch(b)` — shallow clone with `--branch <b>`.
/// - `Tag(t)` — shallow clone with `--branch <t>` (git accepts tag names here).
/// - `Ref(sha)` — clone without depth (a shallow clone cannot fetch an arbitrary
///   sha by default), then fetch the specific commit and check it out.
pub fn clone_at(url: &str, dest: &Path, pin: &Pin) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    match pin {
        Pin::DefaultBranch => {
            run(url, None, &["clone", "--depth", "1", url, &dest_str])?;
        }
        Pin::FollowBranch(branch) => {
            run(
                url,
                None,
                &["clone", "--depth", "1", "--branch", branch, url, &dest_str],
            )?;
        }
        Pin::Tag(tag) => {
            // git clone accepts a tag name as the --branch argument.
            run(
                url,
                None,
                &["clone", "--depth", "1", "--branch", tag, url, &dest_str],
            )?;
        }
        Pin::Ref(sha) => {
            // A shallow clone cannot fetch an arbitrary commit sha because
            // protocol/pack-protocol limits apply. Clone without --depth so
            // all objects are available, then check out the target commit.
            // For file:// and local repos this always works. For real network
            // remotes this costs more bandwidth but is unavoidable unless the
            // server supports `uploadpack.allowReachableSHA1InWant`.
            run(url, None, &["clone", url, &dest_str])?;
            run(url, Some(dest), &["checkout", sha])?;
        }
    }
    Ok(())
}

/// Clone `url` into `dest` (shallow, default branch). Preserved for callers
/// that do not supply a pin (original behavior, same as `clone_at` with
/// `Pin::DefaultBranch`).
pub fn clone(url: &str, dest: &Path) -> Result<()> {
    clone_at(url, dest, &Pin::DefaultBranch)
}

/// Resolve an existing clone against `pin` (CLI-55):
///
/// - `DefaultBranch` — fetch origin's default branch, reset to it.
/// - `FollowBranch(b)` — fetch that branch, reset to it.
/// - `Tag(t)` — force-fetch tags so a re-pointed tag is picked up, then reset
///   to the tag.
/// - `Ref(sha)` — fetch all objects (including the pinned sha if the shallow
///   clone does not have it), then reset to the sha.  The recorded commit
///   stays at `sha` unless the caller changes the pin.
pub fn sync_to_pin(url: &str, dir: &Path, pin: &Pin) -> Result<()> {
    match pin {
        Pin::DefaultBranch => {
            run(url, Some(dir), &["fetch", "--depth", "1", "origin"])?;
            // Reset to whatever origin's HEAD points at.
            let head = run(url, Some(dir), &["rev-parse", "origin/HEAD"]).or_else(|_| {
                // Some remotes don't advertise origin/HEAD after a shallow fetch.
                run(url, Some(dir), &["rev-parse", "FETCH_HEAD"])
            })?;
            run(url, Some(dir), &["reset", "--hard", &head])?;
        }
        Pin::FollowBranch(branch) => {
            run(url, Some(dir), &["fetch", "--depth", "1", "origin", branch])?;
            run(url, Some(dir), &["reset", "--hard", "FETCH_HEAD"])?;
        }
        Pin::Tag(tag) => {
            // Force-fetch tags so a re-pointed tag moves the local ref.
            run(
                url,
                Some(dir),
                &[
                    "fetch",
                    "--force",
                    "origin",
                    &format!("+refs/tags/{tag}:refs/tags/{tag}"),
                ],
            )?;
            run(url, Some(dir), &["reset", "--hard", tag])?;
        }
        Pin::Ref(sha) => {
            // Fetch all to ensure the pinned sha is present (it may be missing
            // if the original clone was shallow).
            run(url, Some(dir), &["fetch", "origin"])?;
            run(url, Some(dir), &["reset", "--hard", sha])?;
        }
    }
    Ok(())
}

/// Resolve the current HEAD commit sha of a clone.
pub fn head_commit(url: &str, dir: &Path) -> Result<String> {
    run(url, Some(dir), &["rev-parse", "HEAD"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn noninteractive_env_disables_git_and_ssh_prompts() {
        // spec: TUI-45 - the non-interactive env makes git fail fast instead of
        // prompting: git's own prompts are off and ssh runs in BatchMode (no
        // passphrase/host-key prompt). A custom base ssh command is preserved.
        let pairs = noninteractive_env_pairs("ssh");
        let map: std::collections::HashMap<_, _> = pairs.iter().cloned().collect();
        assert_eq!(
            map.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        let ssh = map.get("GIT_SSH_COMMAND").expect("GIT_SSH_COMMAND set");
        assert!(
            ssh.contains("BatchMode=yes"),
            "ssh must be BatchMode: {ssh}"
        );
        assert!(ssh.starts_with("ssh "), "base ssh command preserved: {ssh}");

        // A user's custom ssh command is kept as the base.
        let custom = noninteractive_env_pairs("ssh -i /my/key");
        let ssh2 = &custom[1].1;
        assert!(
            ssh2.starts_with("ssh -i /my/key ") && ssh2.contains("BatchMode=yes"),
            "custom base ssh command must be preserved and wrapped: {ssh2}"
        );
    }

    #[test]
    fn set_noninteractive_toggles_the_flag() {
        // spec: TUI-45 - the global flag the TUI flips on (while it owns the
        // terminal) and off (during a suspended interactive meld) round-trips.
        set_noninteractive(true);
        assert!(NONINTERACTIVE.load(Ordering::Relaxed));
        set_noninteractive(false);
        assert!(!NONINTERACTIVE.load(Ordering::Relaxed));
    }

    /// Create a temp directory with a unique name for test isolation.
    fn tmpdir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-git-test-{}-{}-{n}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run `git <args>` in `dir`, panicking on failure. Used to set up fixtures.
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    /// Build a local git repo with a few commits, a branch, and a tag. Returns
    /// `(remote_dir, commit_a_sha, commit_b_sha, commit_c_sha)` where commit A
    /// is the initial commit (tagged `v1.0` and on branch `stable`), B is the
    /// next commit (on `main`), and C is a subsequent commit (on `main` only).
    fn make_remote(base: &Path) -> (PathBuf, String, String, String) {
        let remote = base.join("remote");
        fs::create_dir_all(&remote).unwrap();

        git(&remote, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&remote, &["config", "user.email", "t@t"]);
        git(&remote, &["config", "user.name", "t"]);

        // Commit A
        fs::write(remote.join("file.txt"), "version A").unwrap();
        git(&remote, &["add", "file.txt"]);
        git(&remote, &["commit", "-qm", "commit A"]);
        let sha_a = read_head(&remote);

        // Tag v1.0 at commit A
        git(&remote, &["tag", "v1.0"]);

        // Commit B on main
        fs::write(remote.join("file.txt"), "version B").unwrap();
        git(&remote, &["commit", "-aqm", "commit B"]);
        let sha_b = read_head(&remote);

        // Create `stable` branch pointing at commit A
        git(&remote, &["branch", "stable", &sha_a]);

        // Commit C on main
        fs::write(remote.join("file.txt"), "version C").unwrap();
        git(&remote, &["commit", "-aqm", "commit C"]);
        let sha_c = read_head(&remote);

        (remote, sha_a, sha_b, sha_c)
    }

    fn read_head(dir: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn read_file(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap()
    }

    /// Ensure the clone and drop helpers work at cleanup time.
    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn clone_at_default_branch_checks_out_tip() {
        // spec: CLI-18
        let base = tmpdir("default");
        let (remote, _a, _b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).expect("clone_at default");

        // Should be at the latest commit on main (C)
        let got = read_head(&dest);
        assert_eq!(got, sha_c, "default branch clone should be at tip (C)");
        assert_eq!(read_file(&dest, "file.txt"), "version C");

        cleanup(&base);
    }

    #[test]
    fn clone_at_follow_branch_checks_out_that_branch() {
        // spec: CLI-18
        let base = tmpdir("follow");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::FollowBranch("stable".into())).expect("clone_at stable");

        let got = read_head(&dest);
        assert_eq!(got, sha_a, "follow-branch=stable should be at commit A");
        assert_eq!(read_file(&dest, "file.txt"), "version A");

        cleanup(&base);
    }

    #[test]
    fn clone_at_tag_checks_out_tag() {
        // spec: CLI-18
        let base = tmpdir("tag");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Tag("v1.0".into())).expect("clone_at tag");

        let got = read_head(&dest);
        assert_eq!(got, sha_a, "pin-tag=v1.0 should be at commit A (tagged)");
        assert_eq!(read_file(&dest, "file.txt"), "version A");

        cleanup(&base);
    }

    #[test]
    fn clone_at_ref_checks_out_specific_commit() {
        // spec: CLI-18
        let base = tmpdir("ref");
        let (remote, _a, sha_b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(sha_b.clone())).expect("clone_at ref");

        let got = read_head(&dest);
        assert_eq!(got, sha_b, "pin-ref should land on commit B");
        assert_eq!(read_file(&dest, "file.txt"), "version B");

        cleanup(&base);
    }

    #[test]
    fn sync_follow_branch_moves_to_branch_tip() {
        // spec: CLI-55 — follow-branch resets to the current branch tip
        let base = tmpdir("sync-follow");
        let (remote, sha_a, _b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        // Clone at stable (commit A)
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::FollowBranch("stable".into())).unwrap();
        assert_eq!(read_head(&dest), sha_a);

        // Advance stable branch on the remote to commit C
        git(&remote, &["branch", "-f", "stable", &sha_c]);

        // Sync: stable should now move to C
        sync_to_pin(&url, &dest, &Pin::FollowBranch("stable".into())).unwrap();
        assert_eq!(read_head(&dest), sha_c, "stable after advance should be C");

        cleanup(&base);
    }

    #[test]
    fn sync_pin_ref_stays_fixed() {
        // spec: CLI-55 — pin-ref never moves even when the remote advances
        let base = tmpdir("sync-ref");
        let (remote, _a, sha_b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(sha_b.clone())).unwrap();
        assert_eq!(read_head(&dest), sha_b);

        // The remote already has a commit C; sync with pin-ref still resolves B
        // Add another commit D to make it even clearer
        fs::write(remote.join("file.txt"), "version D").unwrap();
        git(&remote, &["commit", "-aqm", "commit D"]);
        let _ = sha_c; // not used; just ensuring it is different from sha_b

        sync_to_pin(&url, &dest, &Pin::Ref(sha_b.clone())).unwrap();
        assert_eq!(read_head(&dest), sha_b, "pin-ref must stay fixed on sync");

        cleanup(&base);
    }

    #[test]
    fn sync_pin_tag_moves_when_tag_is_moved() {
        // spec: CLI-55 — pin-tag re-fetches and resets; a moved tag is picked up
        let base = tmpdir("sync-tag");
        let (remote, sha_a, _b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Tag("v1.0".into())).unwrap();
        assert_eq!(read_head(&dest), sha_a);

        // Move v1.0 to point at commit C on the remote
        git(&remote, &["tag", "-f", "v1.0", &sha_c]);

        sync_to_pin(&url, &dest, &Pin::Tag("v1.0".into())).unwrap();
        assert_eq!(
            read_head(&dest),
            sha_c,
            "pin-tag with moved tag should advance to C"
        );

        cleanup(&base);
    }

    #[test]
    fn sync_pin_tag_stays_when_tag_is_not_moved() {
        // spec: CLI-55 — pin-tag with a fixed (unmoved) tag stays at original commit
        let base = tmpdir("sync-tag-fixed");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Tag("v1.0".into())).unwrap();
        assert_eq!(read_head(&dest), sha_a);

        // Remote advances but the tag v1.0 stays at A; add another commit
        fs::write(remote.join("file.txt"), "version D").unwrap();
        git(&remote, &["commit", "-aqm", "commit D"]);

        // Sync: tag not moved => stays at A
        sync_to_pin(&url, &dest, &Pin::Tag("v1.0".into())).unwrap();
        assert_eq!(
            read_head(&dest),
            sha_a,
            "pin-tag with unmoved tag should stay at A"
        );

        cleanup(&base);
    }
}
