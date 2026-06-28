//! Thin wrapper around the `git` CLI, surfacing failures as [`MindError::Git`].

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{MindError, Result};
use crate::source::Pin;

/// Reject a pin/ref value that starts with `-` or is otherwise unsafe to pass
/// as a positional git argument (DSC-66).
///
/// Rules (conservative, safe subset):
/// - Must not be empty.
/// - Must not begin with `-` (would be interpreted as a git option).
/// - Must not contain ASCII whitespace (shells/git split on whitespace; no ref
///   name contains internal spaces by git convention).
/// - Must not contain `..` (git range syntax, would be interpreted as a range
///   rather than a single ref).
/// - Must not contain NUL or other ASCII control characters.
///
/// These rules are not exhaustive of every invalid git ref name (see
/// `git-check-ref-format(1)`), but they catch the injection-relevant cases while
/// accepting every branch name, tag, and full SHA a legitimate caller would supply.
pub fn validate_ref_value(value: &str) -> Result<()> {
    // spec: DSC-66
    if value.is_empty() {
        return Err(MindError::InvalidRef {
            value: value.to_string(),
            reason: "ref value must not be empty".to_string(),
        });
    }
    if value.starts_with('-') {
        return Err(MindError::InvalidRef {
            value: value.to_string(),
            reason: "ref value must not begin with '-' (looks like a git option)".to_string(),
        });
    }
    if value.chars().any(|c| c.is_ascii_control()) {
        return Err(MindError::InvalidRef {
            value: value.to_string(),
            reason: "ref value must not contain control characters".to_string(),
        });
    }
    if value.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(MindError::InvalidRef {
            value: value.to_string(),
            reason: "ref value must not contain whitespace".to_string(),
        });
    }
    if value.contains("..") {
        return Err(MindError::InvalidRef {
            value: value.to_string(),
            reason: "ref value must not contain '..' (ambiguous git range syntax)".to_string(),
        });
    }
    Ok(())
}

/// Detect whether a [`MindError`] is an authentication failure from a git
/// subprocess. Returns true when `err` is a [`MindError::Git`] whose stderr
/// matches at least one known credential-denial pattern (case-insensitive).
///
/// These patterns cover the common auth-failure messages from GitHub, GitLab,
/// Bitbucket, and generic HTTP remotes over HTTPS and SSH.
pub fn is_auth_failure(err: &MindError) -> bool {
    // spec: DSC-68
    let stderr = match err {
        MindError::Git { stderr, .. } => stderr.to_lowercase(),
        _ => return false,
    };
    const PATTERNS: &[&str] = &[
        "authentication failed",
        "permission denied (publickey)",
        "could not read username",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
        "invalid username or password",
        "invalid credentials",
        "fatal: unable to authenticate",
    ];
    PATTERNS.iter().any(|p| stderr.contains(p))
}

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
            // --branch consumes its next argument as a value, not a positional,
            // so it is already safe.  Validate anyway (DSC-66) so a bad value
            // is caught at parse time before reaching any subprocess.
            run(
                url,
                None,
                &["clone", "--depth", "1", "--branch", branch, url, &dest_str],
            )?;
        }
        Pin::Tag(tag) => {
            // git clone accepts a tag name as the --branch argument (value, not
            // positional).  Validate (DSC-66) for the same reason as above.
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
            //
            // `git checkout <commit>` does not accept `--` before the commit
            // argument (that form means "path operands follow"). Injection
            // safety is provided by `validate_ref_value` at parse time (DSC-66),
            // which rejects any value starting with `-` before it reaches here.
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
            // `head` is the output of rev-parse (a full SHA), not user input.
            // `git reset --hard` does not accept `--` before the commit argument
            // (that form switches to path mode); injection safety here relies on
            // the value being a raw hex SHA from rev-parse, not user-supplied.
            run(url, Some(dir), &["reset", "--hard", &head])?;
        }
        Pin::FollowBranch(branch) => {
            // Insert `--` after `origin` so the branch name is always treated as
            // a refspec operand, never as an option (DSC-66). This is the correct
            // end-of-options form for `git fetch`.
            run(
                url,
                Some(dir),
                &["fetch", "--depth", "1", "origin", "--", branch],
            )?;
            // FETCH_HEAD is an internal git ref, not user-supplied.
            run(url, Some(dir), &["reset", "--hard", "FETCH_HEAD"])?;
        }
        Pin::Tag(tag) => {
            // Force-fetch tags so a re-pointed tag moves the local ref.
            // The refspec `+refs/tags/{tag}:refs/tags/{tag}` already encodes the
            // tag name inside a fixed-prefix string; the `--` before the refspec
            // guards the positional argument position (DSC-66).
            run(
                url,
                Some(dir),
                &[
                    "fetch",
                    "--force",
                    "origin",
                    "--",
                    &format!("+refs/tags/{tag}:refs/tags/{tag}"),
                ],
            )?;
            // `git reset --hard <tree-ish>`: the commit argument is positional
            // and NOT preceded by `--` because that form means "paths follow",
            // not "end of options". Injection safety is from `validate_ref_value`
            // at parse time (DSC-66), which rejects any value starting with `-`.
            run(url, Some(dir), &["reset", "--hard", tag])?;
        }
        Pin::Ref(sha) => {
            // Fetch all to ensure the pinned sha is present (it may be missing
            // if the original clone was shallow).
            run(url, Some(dir), &["fetch", "origin"])?;
            // Same as Tag: no `--` before the commit; injection safety comes
            // from `validate_ref_value` at parse time (DSC-66).
            run(url, Some(dir), &["reset", "--hard", sha])?;
        }
    }
    Ok(())
}

/// Resolve the current HEAD commit sha of a clone.
pub fn head_commit(url: &str, dir: &Path) -> Result<String> {
    run(url, Some(dir), &["rev-parse", "HEAD"])
}

/// Initialize a new git repository at `dir`, creating the directory if absent.
pub fn git_init(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| crate::error::MindError::io(dir, e))?;
    run(&dir.to_string_lossy(), Some(dir), &["init", "-q"])?;
    // Set minimal identity so commits work without a global git config.
    run(
        &dir.to_string_lossy(),
        Some(dir),
        &["config", "user.email", "mind@local"],
    )?;
    run(
        &dir.to_string_lossy(),
        Some(dir),
        &["config", "user.name", "mind"],
    )?;
    Ok(())
}

/// Return `true` when `dir` is inside a git repository (i.e. `git rev-parse
/// --git-dir` succeeds). Used to validate absorb destinations (ABS-5).
pub fn is_repo(dir: &Path) -> bool {
    run(
        &dir.to_string_lossy(),
        Some(dir),
        &["rev-parse", "--git-dir"],
    )
    .is_ok()
}

/// Stage all changes under `dir` (`git add -A`).
pub fn add_all(dir: &Path) -> Result<()> {
    run(&dir.to_string_lossy(), Some(dir), &["add", "-A"])?;
    Ok(())
}

/// Create a commit in `dir` with `message`. Errors if there is nothing staged.
pub fn commit(dir: &Path, message: &str) -> Result<()> {
    run(
        &dir.to_string_lossy(),
        Some(dir),
        &["commit", "-m", message],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    // ---- DSC-66: validate_ref_value ----------------------------------------

    #[test]
    fn validate_ref_value_accepts_normal_refs() {
        // spec: DSC-66 - well-formed branch names, tags, and SHAs pass validation.
        for good in [
            "main",
            "develop",
            "release/2.0",
            "v1.0",
            "v1.0.0-rc1",
            "feature/abc-123",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "abc1234",
        ] {
            assert!(validate_ref_value(good).is_ok(), "expected ok for {good:?}");
        }
    }

    #[test]
    fn validate_ref_value_boundary_dash_and_colon_and_refspec() {
        // spec: DSC-66 - boundary cases the happy-path accept list does not pin.
        //
        // 1. A value that is *exactly* "-" is rejected (it is the leading-dash
        //    class with nothing after it; git would read it as an option/stdin).
        let err = validate_ref_value("-").unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "a bare '-' must be rejected, got: {err}"
        );

        // 2. A dash that is NOT leading must be ACCEPTED: the rule targets only
        //    git-option-looking values, not legitimate refs containing '-'.
        for good in ["x-y", "abc-", "feature-1", "a-b-c"] {
            assert!(
                validate_ref_value(good).is_ok(),
                "non-leading dash must be allowed: {good:?}"
            );
        }

        // 3. A full refspec-looking ref like refs/tags/v1 is a legitimate ref and
        //    must be ACCEPTED (slashes are fine).
        for good in [
            "refs/tags/v1",
            "refs/heads/main",
            "refs/remotes/origin/main",
        ] {
            assert!(
                validate_ref_value(good).is_ok(),
                "refspec-looking ref must be allowed: {good:?}"
            );
        }
    }

    #[test]
    fn validate_ref_value_accepts_colon_documenting_current_contract() {
        // spec: DSC-66 - DSC-66 enumerates the rejected classes (empty,
        // leading '-', whitespace, control chars, '..'). ':' is deliberately
        // NOT in that set, so a colon-containing value is ACCEPTED. This is
        // safe: every git call site either passes the value after a '--'
        // end-of-options terminator (fetch) or as a positional tree-ish to
        // checkout/reset where a ':' cannot form an option (the parse-time
        // leading-dash check is the option barrier). A ':' could only corrupt a
        // refspec into a src:dst form, which git rejects as an unknown ref - a
        // correctness error surfaced by git, not an injection. This test pins
        // the current contract so any future change (rejecting ':' or, worse,
        // silently mangling it) is caught and forces a spec decision.
        for accepted in ["a:b", "refs/tags/v1:refs/tags/v1", "feature/x:y"] {
            assert!(
                validate_ref_value(accepted).is_ok(),
                "':' is not in the DSC-66 rejected set; {accepted:?} must pass"
            );
        }
    }

    #[test]
    fn validate_ref_value_rejects_empty() {
        // spec: DSC-66 - empty value is always rejected.
        let err = validate_ref_value("").unwrap_err();
        assert!(
            err.to_string().contains("empty"),
            "error should mention 'empty': {err}"
        );
    }

    #[test]
    fn validate_ref_value_rejects_leading_dash() {
        // spec: DSC-66 - a leading `-` looks like a git option and is rejected.
        for bad in [
            "--upload-pack=touch /tmp/pwned",
            "-x",
            "--no-tags",
            "--depth=1",
        ] {
            let err = validate_ref_value(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::InvalidRef { .. }),
                "expected InvalidRef for {bad:?}, got: {err}"
            );
            assert!(
                err.to_string().contains("'-'"),
                "error should mention '-': {err}"
            );
        }
    }

    #[test]
    fn validate_ref_value_rejects_whitespace() {
        // spec: DSC-66 - whitespace in a ref value is rejected (spaces and tabs).
        // Space is ASCII whitespace but not a control character; a value with a
        // space is caught by the whitespace check and the error message says
        // "whitespace".  Tab is an ASCII control character so it is caught
        // earlier (the control-char check) and the message says "control
        // characters".  Both must produce an InvalidRef error.
        for bad in ["main branch", "v1.0 stable", "a b"] {
            let err = validate_ref_value(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::InvalidRef { .. }),
                "expected InvalidRef for {bad:?}"
            );
            assert!(
                err.to_string().contains("whitespace"),
                "error should mention 'whitespace': {err}"
            );
        }
        // Tab is caught by the control-char check (runs first).
        let err = validate_ref_value("ref\twith\ttabs").unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef for tab-containing value"
        );
    }

    #[test]
    fn validate_ref_value_rejects_dotdot() {
        // spec: DSC-66 - '..' is git range syntax and is rejected.
        for bad in ["main..HEAD", "v1.0..v2.0", "a..b", "..HEAD"] {
            let err = validate_ref_value(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::InvalidRef { .. }),
                "expected InvalidRef for {bad:?}"
            );
            assert!(
                err.to_string().contains(".."),
                "error should mention '..': {err}"
            );
        }
    }

    #[test]
    fn validate_ref_value_rejects_control_chars() {
        // spec: DSC-66 - ASCII control characters are rejected.
        let nul = "\x00ref";
        let err = validate_ref_value(nul).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef for NUL-containing value"
        );
    }

    #[test]
    fn clone_at_ref_pin_succeeds_with_validated_sha() {
        // spec: DSC-66 - `clone_at` with a Pin::Ref succeeds for a valid SHA
        // against a local fixture repo.  Injection safety relies on
        // `validate_ref_value` at parse time (no `--` in `git checkout <sha>`
        // because that form would switch to path mode).
        let base = tmpdir("dsc66-checkout");
        let (remote, _a, sha_b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(sha_b.clone()))
            .expect("clone_at Pin::Ref must succeed for a valid sha");

        let got = read_head(&dest);
        assert_eq!(got, sha_b, "clone_at must land on the pinned sha");

        cleanup(&base);
    }

    #[test]
    fn sync_to_pin_follow_branch_with_fetch_double_dash_succeeds() {
        // spec: DSC-66 - the `--` inserted between `origin` and the branch name
        // in `fetch --depth 1 origin -- <branch>` does not break the fetch.
        // This is the one git subcommand where `--` correctly ends options
        // before the refspec operand.
        let base = tmpdir("dsc66-fetch-branch");
        let (remote, sha_a, _b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::FollowBranch("stable".into())).unwrap();
        assert_eq!(read_head(&dest), sha_a);

        git(&remote, &["branch", "-f", "stable", &sha_c]);

        sync_to_pin(&url, &dest, &Pin::FollowBranch("stable".into()))
            .expect("fetch with -- terminator must succeed");
        assert_eq!(
            read_head(&dest),
            sha_c,
            "-- before the branch name must not disturb the fetch"
        );

        cleanup(&base);
    }

    #[test]
    fn sync_to_pin_ref_succeeds_with_validated_sha() {
        // spec: DSC-66 - `sync_to_pin` with Pin::Ref succeeds for a valid SHA.
        // Injection safety relies on `validate_ref_value` at parse time; there
        // is no `--` before the sha in `git reset --hard <sha>` because that
        // form switches reset to path mode.
        let base = tmpdir("dsc66-reset-ref");
        let (remote, _a, sha_b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(sha_b.clone())).unwrap();
        assert_eq!(read_head(&dest), sha_b);

        // Add a new commit so the remote has advanced.
        fs::write(remote.join("file.txt"), "version D").unwrap();
        git(&remote, &["commit", "-aqm", "commit D"]);

        sync_to_pin(&url, &dest, &Pin::Ref(sha_b.clone()))
            .expect("sync_to_pin Pin::Ref must succeed for a valid sha");
        assert_eq!(
            read_head(&dest),
            sha_b,
            "sync_to_pin must stay on the pinned sha"
        );

        cleanup(&base);
    }

    #[test]
    fn sync_to_pin_tag_with_fetch_double_dash_succeeds() {
        // spec: DSC-66 - `sync_to_pin` with Pin::Tag succeeds.  The tag refspec
        // is passed after `--` in the `git fetch` call (correct form); the
        // `git reset --hard <tag>` call does NOT use `--` (that would switch to
        // path mode).  Injection safety for the reset comes from
        // `validate_ref_value` at parse time.
        let base = tmpdir("dsc66-reset-tag");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Tag("v1.0".into())).unwrap();
        assert_eq!(read_head(&dest), sha_a);

        sync_to_pin(&url, &dest, &Pin::Tag("v1.0".into()))
            .expect("tag sync with -- in fetch must succeed");
        assert_eq!(
            read_head(&dest),
            sha_a,
            "sync_to_pin must stay on the pinned tag"
        );

        cleanup(&base);
    }

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

    // ---- absorb git helpers (ABS-5) -----------------------------------------
    // These exercise the actual `crate::git` functions, not the git CLI used to
    // build fixtures. The integration suite only reaches these via the absorb
    // command; here we pin their contract directly.

    /// `git_init` creates the dir if absent, initializes a repo (`is_repo` true),
    /// and sets a usable identity so a later `commit` works with no global config.
    // spec: ABS-5
    #[test]
    fn git_init_creates_repo_with_identity() {
        let base = tmpdir("ginit");
        let repo = base.join("nested").join("repo");
        // The directory does not exist yet; git_init must create it.
        assert!(!repo.exists(), "sanity: repo dir must not pre-exist");

        git_init(&repo).expect("git_init");
        assert!(repo.exists(), "git_init must create the directory");
        assert!(is_repo(&repo), "git_init must produce a git repo");

        cleanup(&base);
    }

    /// `is_repo` is false for a plain directory and true after `git_init`.
    // spec: ABS-5
    #[test]
    fn is_repo_false_for_plain_dir_true_after_init() {
        let base = tmpdir("isrepo");
        let plain = base.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert!(!is_repo(&plain), "a plain dir is not a git repo");

        git_init(&plain).unwrap();
        assert!(is_repo(&plain), "after git_init the dir is a git repo");

        cleanup(&base);
    }

    /// `is_repo` is false for a path that does not exist at all.
    // spec: ABS-5
    #[test]
    fn is_repo_false_for_missing_path() {
        let base = tmpdir("isrepo-missing");
        let missing = base.join("does-not-exist");
        assert!(
            !is_repo(&missing),
            "a non-existent path must not report as a git repo"
        );
        cleanup(&base);
    }

    /// `add_all` then `commit` records a commit with the given message, and a
    /// fresh `git_init`'d repo can commit a newly written file (the absorb flow:
    /// move file in, add_all, commit "absorb kind:name").
    // spec: ABS-5
    #[test]
    fn add_all_and_commit_records_message() {
        let base = tmpdir("commit");
        let repo = base.join("repo");
        git_init(&repo).unwrap();

        // Write a file (as absorb does after the move) then stage + commit.
        fs::write(repo.join("skill.md"), "# absorbed\n").unwrap();
        add_all(&repo).expect("add_all");
        commit(&repo, "absorb skill:review").expect("commit");

        let msg = read_head_subject(&repo);
        assert_eq!(
            msg, "absorb skill:review",
            "commit message must be the absorb default message"
        );
        // The committed tree contains the staged file.
        let tracked = Command::new("git")
            .args(["ls-files"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let tracked = String::from_utf8(tracked.stdout).unwrap();
        assert!(
            tracked.contains("skill.md"),
            "the committed file must be tracked: {tracked}"
        );

        cleanup(&base);
    }

    /// `commit` with nothing staged is an error: the absorb flow never commits an
    /// empty change, and a misuse surfaces as a failure rather than a silent no-op.
    // spec: ABS-5
    #[test]
    fn commit_with_nothing_staged_errors() {
        let base = tmpdir("commit-empty");
        let repo = base.join("repo");
        git_init(&repo).unwrap();
        // Nothing written/staged after init.
        let result = commit(&repo, "absorb skill:nothing");
        assert!(
            result.is_err(),
            "commit with an empty index must error, not produce an empty commit"
        );
        cleanup(&base);
    }

    /// Read the subject (`%s`) of HEAD in `dir`.
    fn read_head_subject(dir: &Path) -> String {
        let out = Command::new("git")
            .args(["log", "-1", "--pretty=format:%s"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    // ---- DSC-68: is_auth_failure detection ----

    /// Build a [`MindError::Git`] carrying `stderr`, for auth-detection tests.
    fn git_err(stderr: &str) -> MindError {
        MindError::Git {
            url: "https://example.com/repo.git".into(),
            args: vec!["clone".into()],
            status: None,
            stderr: stderr.into(),
        }
    }

    #[test]
    fn is_auth_failure_matches_authentication_failed() {
        // spec: DSC-68
        let err = git_err("fatal: Authentication failed for 'https://github.com/owner/private/'");
        assert!(is_auth_failure(&err), "authentication failed must match");
    }

    #[test]
    fn is_auth_failure_matches_permission_denied_publickey() {
        // spec: DSC-68
        let err = git_err("git@github.com: Permission denied (publickey).");
        assert!(
            is_auth_failure(&err),
            "Permission denied (publickey) must match"
        );
    }

    #[test]
    fn is_auth_failure_matches_http_401() {
        // spec: DSC-68
        let err = git_err(
            "fatal: unable to access 'https://example.com/private.git/': The requested URL returned error: 401",
        );
        assert!(is_auth_failure(&err), "401 error must match");
    }

    #[test]
    fn is_auth_failure_matches_http_403() {
        // spec: DSC-68
        let err = git_err(
            "fatal: unable to access 'https://example.com/private.git/': The requested URL returned error: 403",
        );
        assert!(is_auth_failure(&err), "403 error must match");
    }

    #[test]
    fn is_auth_failure_does_not_match_repository_not_found() {
        // spec: DSC-68
        // "repository not found" is deliberately excluded: it conflates private
        // repos (where the server hides existence behind a 404) with repos that
        // are genuinely missing or have been deleted. Treating it as an auth
        // failure would cause false prompts for repos that simply do not exist.
        let err = git_err("ERROR: Repository not found.");
        assert!(
            !is_auth_failure(&err),
            "Repository not found must not match"
        );
    }

    #[test]
    fn is_auth_failure_matches_invalid_username_or_password() {
        // spec: DSC-68
        let err = git_err("remote: Invalid username or password.");
        assert!(
            is_auth_failure(&err),
            "Invalid username or password must match"
        );
    }

    #[test]
    fn is_auth_failure_matches_could_not_read_username() {
        // spec: DSC-68
        let err = git_err(
            "fatal: could not read Username for 'https://github.com': No such device or address",
        );
        assert!(is_auth_failure(&err), "could not read Username must match");
    }

    #[test]
    fn is_auth_failure_matches_invalid_credentials() {
        // spec: DSC-68
        let err = git_err("Invalid credentials.");
        assert!(is_auth_failure(&err), "invalid credentials must match");
    }

    #[test]
    fn is_auth_failure_matches_unable_to_authenticate() {
        // spec: DSC-68
        let err = git_err("fatal: unable to authenticate");
        assert!(is_auth_failure(&err), "unable to authenticate must match");
    }

    #[test]
    fn is_auth_failure_does_not_match_network_error() {
        // spec: DSC-68 -- a plain network failure is not an auth failure
        let err = git_err("fatal: unable to connect to github.com: Connection refused");
        assert!(
            !is_auth_failure(&err),
            "a network error must not match auth failure"
        );
    }

    #[test]
    fn is_auth_failure_does_not_match_non_git_error() {
        // spec: DSC-68 -- a non-Git MindError is never an auth failure
        let err = MindError::GitNotFound;
        assert!(
            !is_auth_failure(&err),
            "GitNotFound must not be an auth failure"
        );
    }

    #[test]
    fn is_auth_failure_is_case_insensitive() {
        // spec: DSC-68 -- pattern matching is case-insensitive
        let err = git_err("AUTHENTICATION FAILED for something");
        assert!(
            is_auth_failure(&err),
            "auth failure detection must be case-insensitive"
        );
    }
}
