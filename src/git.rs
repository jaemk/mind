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
/// These patterns cover the common auth-failure messages from GitHub, GitLab
/// (`HTTP Basic: Access denied`), Bitbucket, and generic HTTP/SSH remotes.
pub fn is_auth_failure(err: &MindError) -> bool {
    // spec: DSC-68
    let stderr = match err {
        MindError::Git { stderr, .. } => stderr.to_ascii_lowercase(),
        _ => return false,
    };
    const PATTERNS: &[&str] = &[
        "authentication failed",
        "permission denied (publickey)",
        "could not read username",
        "could not read password",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
        "invalid username or password",
        "invalid credentials",
        "http basic: access denied",
        "fatal: unable to authenticate",
    ];
    PATTERNS.iter().any(|p| stderr.contains(p))
}

/// Detect whether a [`MindError`] is a proxy interception from a git subprocess.
/// Returns true when `err` is a [`MindError::Git`] whose stderr matches at least
/// one known corporate-proxy failure pattern (case-insensitive).
///
/// These patterns cover the three most common proxy failure messages:
/// - `Received HTTP code 407 from proxy` (git HTTP backend)
/// - `The requested URL returned error: 407` (curl backend)
/// - `Could not resolve proxy` (DNS failure reaching the proxy host itself)
pub fn is_proxy_failure(err: &MindError) -> bool {
    // spec: CLI-178
    let stderr = match err {
        MindError::Git { stderr, .. } => stderr.to_ascii_lowercase(),
        _ => return false,
    };
    const PATTERNS: &[&str] = &[
        "received http code 407",
        "the requested url returned error: 407",
        "could not resolve proxy",
    ];
    PATTERNS.iter().any(|p| stderr.contains(p))
}

/// Build the hint lines to print when a top-level `meld` or `sync` operation
/// fails with an authentication error (CLI-177). The three suggested remedies are:
///   1. Retry using the SSH remote form directly.
///   2. Set `ssh = true` in `~/.mind/config.toml` so all HTTPS remotes use SSH.
///   3. Configure a git credential helper.
///
/// `ssh_url` is the `git@host:owner/repo` form of the failing remote.
pub fn auth_hint_lines(ssh_url: &str) -> Vec<String> {
    // spec: CLI-177
    vec![
        // spec: CLI-177 -- GitHub (and most forges) answer an unauthenticated
        // request for a missing, private, or mistyped repo with the same
        // credential prompt as a real auth failure, so `is_auth_failure` cannot
        // tell the two apart. Lead with this note so a typo'd repo name is not
        // sent straight into SSH-key setup before checking the simpler cause.
        "note: this can also mean the repo does not exist, is private, or the name is \
         misspelled -- double check the identity first"
            .to_string(),
        "hint: authentication failed; to use SSH:".to_string(),
        format!("  mind meld {ssh_url}"),
        "  or set `ssh = true` in ~/.mind/config.toml to always prefer SSH for HTTPS remotes"
            .to_string(),
        "  or configure a git credential helper: https://git-scm.com/doc/credential-helpers"
            .to_string(),
    ]
}

/// Build the hint lines to print when a `meld` or `sync` git operation fails
/// with a proxy error (CLI-178). Mirrors the wording used by `evolve` for the
/// same class of failure.
pub fn proxy_hint_lines() -> Vec<String> {
    // spec: CLI-178
    vec![
        "hint: a proxy may be blocking the connection; \
         set HTTPS_PROXY (e.g. export HTTPS_PROXY=http://proxy.example.com:8080) \
         or configure git's http.proxy setting"
            .to_string(),
    ]
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

/// Whether the process-wide non-interactive git mode is on.
pub fn is_noninteractive() -> bool {
    NONINTERACTIVE.load(Ordering::Relaxed)
}

thread_local! {
    /// Per-thread non-interactive mode, for git children spawned by a parallel
    /// fetch worker: concurrent prompts on one terminal would interleave, so a
    /// worker never prompts (CLI-248).
    static THREAD_NONINTERACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with every `git` child it spawns on this thread non-interactive.
pub fn with_noninteractive<R>(f: impl FnOnce() -> R) -> R {
    let prev = THREAD_NONINTERACTIVE.with(|c| c.replace(true));
    let out = f();
    THREAD_NONINTERACTIVE.with(|c| c.set(prev));
    out
}

/// Whether `err` is a failure a non-interactive git child reports in place of
/// a prompt it was not allowed to show: a credential or passphrase prompt
/// (see [`is_auth_failure`]) or an ssh host-key confirmation.
pub fn is_suppressed_prompt_failure(err: &MindError) -> bool {
    is_auth_failure(err)
        || matches!(err, MindError::Git { stderr, .. }
            if stderr.to_ascii_lowercase().contains("host key verification failed"))
}

/// The env pairs that make a `git` child non-interactive. `GIT_TERMINAL_PROMPT=0`
/// stops git's own credential prompts; the prompt-free ssh command (see
/// [`prompt_free_ssh`]) stops ssh's passphrase and host-key prompts, and a
/// short `ConnectTimeout` avoids a long network hang.
// spec: TUI-45
fn noninteractive_env_pairs(ssh: &SshSetup) -> Vec<(&'static str, String)> {
    prompt_free_env(ssh, "-o BatchMode=yes -o ConnectTimeout=10")
}

/// `GIT_TERMINAL_PROMPT=0`, plus `GIT_SSH_COMMAND` when `ssh` has a
/// prompt-free form; OpenSSH gets `openssh_opts`.
fn prompt_free_env(ssh: &SshSetup, openssh_opts: &str) -> Vec<(&'static str, String)> {
    let mut pairs = vec![("GIT_TERMINAL_PROMPT", "0".to_string())];
    if let Some(command) = prompt_free_ssh(ssh, openssh_opts) {
        pairs.push(("GIT_SSH_COMMAND", command));
    }
    pairs
}

/// The ssh command and variant a `git` child would use.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SshSetup {
    /// A shell command, as `GIT_SSH_COMMAND` takes it.
    command: String,
    /// `GIT_SSH_VARIANT` or `ssh.variant`, when set.
    variant: Option<String>,
}

/// `ssh`'s command with prompts off, or `None` to leave it as configured.
/// The variant is git's own: an explicit one other than `auto` wins, else the
/// command's program name. OpenSSH takes `openssh_opts`, plink and putty take
/// `-batch`. TortoisePlink already gets `-batch` from git, and any other
/// program (a wrapper script, `simple`) may not accept either flag, so it is
/// left alone rather than broken.
// spec: PRG-6 TUI-45
fn prompt_free_ssh(ssh: &SshSetup, openssh_opts: &str) -> Option<String> {
    let variant = match ssh.variant.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() && !v.eq_ignore_ascii_case("auto") => v.to_ascii_lowercase(),
        _ => program_name(&ssh.command),
    };
    let extra = match variant.as_str() {
        "ssh" => openssh_opts,
        "plink" | "putty" => "-batch",
        _ => return None,
    };
    Some(format!("{} {extra}", ssh.command))
}

/// The lowercase base name, `.exe` dropped, of a shell command's first word
/// (single- or double-quoted, or bare).
fn program_name(command: &str) -> String {
    let command = command.trim_start();
    let word = match command.chars().next() {
        Some(q @ ('\'' | '"')) => command[1..].split(q).next().unwrap_or(""),
        _ => command.split_whitespace().next().unwrap_or(""),
    };
    let base = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    base.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(base)
}

/// Whether this thread's `git` children run non-interactive (the process-wide
/// or the per-thread mode).
fn noninteractive_here() -> bool {
    NONINTERACTIVE.load(Ordering::Relaxed) || THREAD_NONINTERACTIVE.with(|c| c.get())
}

/// The ssh setup a `git` child in `cwd` would use, which the non-interactive
/// and batch wrappers extend. Setting `GIT_SSH_COMMAND` overrides
/// `core.sshCommand` and `GIT_SSH`, so the wrapper has to start from whichever
/// of them git would have used, or a configured key or proxy is silently
/// dropped.
fn ssh_setup(cwd: Option<&Path>) -> SshSetup {
    let var = |k| {
        std::env::var(k)
            .ok()
            .filter(|v: &String| !v.trim().is_empty())
    };
    let config = config_ssh(cwd);
    SshSetup {
        command: resolve_ssh_base(
            var("GIT_SSH_COMMAND"),
            || config.command.clone(),
            var("GIT_SSH"),
        ),
        variant: var("GIT_SSH_VARIANT").or(config.variant),
    }
}

/// git's own order (`connect.c`): `GIT_SSH_COMMAND`, then `core.sshCommand`,
/// then the `GIT_SSH` program, then `ssh`. An empty value counts as unset.
/// `GIT_SSH` names a program, not a shell command, so it is quoted.
fn resolve_ssh_base(
    env_command: Option<String>,
    config_command: impl FnOnce() -> Option<String>,
    git_ssh: Option<String>,
) -> String {
    let set = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    set(env_command)
        .or_else(|| set(config_command()))
        .or_else(|| set(git_ssh).map(|p| format!("'{}'", p.replace('\'', r"'\''"))))
        .unwrap_or_else(|| "ssh".to_string())
}

/// `core.sshCommand` and `ssh.variant` as git config holds them.
#[derive(Debug, Default, PartialEq, Eq)]
struct ConfigSsh {
    command: Option<String>,
    variant: Option<String>,
}

/// [`ConfigSsh`] as a `git` child in `cwd` sees it, in one `git config`
/// call. With no `cwd` (a clone) it is read outside any repository, so only
/// the system and global config count, as for the clone itself.
fn config_ssh(cwd: Option<&Path>) -> ConfigSsh {
    let dir = cwd.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let Ok(out) = Command::new("git")
        .args([
            "config",
            "--get-regexp",
            r"^(core\.sshcommand|ssh\.variant)$",
        ])
        .current_dir(dir)
        .output()
    else {
        return ConfigSsh::default();
    };
    parse_config_ssh(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `git config --get-regexp` lines (`<key> <value>`, the key
/// lowercased). A later line wins, as the last value does for git.
fn parse_config_ssh(text: &str) -> ConfigSsh {
    let mut found = ConfigSsh::default();
    for line in text.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let value = Some(value.trim().to_string()).filter(|v| !v.is_empty());
        match key {
            "core.sshcommand" => found.command = value,
            "ssh.variant" => found.variant = value,
            _ => {}
        }
    }
    found
}

/// Apply the non-interactive environment to a `git` child in `cwd` when the
/// mode is on. In the process-wide mode (the TUI), an ssh program with no
/// prompt-free form loses its controlling terminal instead, so a prompt it
/// would draw fails at once rather than hang the UI.
fn apply_noninteractive_env(cmd: &mut Command, cwd: Option<&Path>) {
    if !noninteractive_here() {
        return;
    }
    let ssh = ssh_setup(cwd);
    for (k, v) in noninteractive_env_pairs(&ssh) {
        cmd.env(k, v);
    }
    if must_detach(&ssh, is_noninteractive()) {
        detach_from_terminal(cmd);
    }
}

/// Whether a non-interactive `git` child must lose its terminal: only in the
/// process-wide mode, and only when `ssh` has no prompt-free form. A parallel
/// fetch worker keeps it, so Ctrl-C still reaches its children.
// spec: TUI-45
fn must_detach(ssh: &SshSetup, process_wide: bool) -> bool {
    process_wide && prompt_free_ssh(ssh, "").is_none()
}

/// Run `cmd` in a new session with no controlling terminal, so nothing it
/// spawns can open `/dev/tty` to prompt (TUI-45). Its stdin is null.
// spec: TUI-45
fn detach_from_terminal(cmd: &mut Command) {
    cmd.stdin(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state; it
        // only detaches the child from the parent's session and terminal.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
}

/// The env pairs for a `git` child run while a spinner may be drawn (PRG-6):
/// the non-interactive pairs without the short `ConnectTimeout`, since the
/// attempt is retried with prompts on rather than failed, and a slow host
/// should not turn into a timeout.
// spec: PRG-6
fn batch_env_pairs(ssh: &SshSetup) -> Vec<(&'static str, String)> {
    prompt_free_env(ssh, "-o BatchMode=yes")
}

/// Run `attempt`, first with prompts off when `batch` is set; a failure that
/// stands in for a prompt git was not allowed to show is retried once with
/// prompts on and the spinner cleared (PRG-6).
// spec: PRG-6
fn with_prompt_retry<T>(batch: bool, mut attempt: impl FnMut(bool) -> Result<T>) -> Result<T> {
    match attempt(batch) {
        Err(e) if batch && is_suppressed_prompt_failure(&e) => {
            crate::progress::suspend(|| attempt(false))
        }
        r => r,
    }
}

/// The spinner label for a `git` subcommand that talks to the remote, or
/// `None` for a local one (PRG-4).
// spec: PRG-4
fn network_label(url: &str, args: &[&str]) -> Option<String> {
    let verb = match args.first().copied()? {
        "clone" => "cloning",
        "fetch" => "fetching",
        "ls-remote" => "listing tags of",
        _ => return None,
    };
    Some(format!("{verb} {url}"))
}

/// Run `git <args>` in `cwd`, returning trimmed stdout on success. A call
/// that reaches the remote shows a progress step (PRG-4), and while progress
/// is on it runs with prompts off first (PRG-6).
fn run(url: &str, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let label = network_label(url, args);
    // A local subcommand never prompts, so only a remote one gets the
    // prompt-free env.
    let remote = label.is_some();
    let batch = remote && crate::progress::enabled() && !noninteractive_here();
    let _step = label.map(|label| crate::progress::step_if_idle(|| label));
    with_prompt_retry(batch, |batch| exec(url, cwd, args, remote, batch))
}

/// One `git <args>` run. A `remote` one gets the non-interactive env when
/// that mode is on; `batch` applies [`batch_env_pairs`].
fn exec(url: &str, cwd: Option<&Path>, args: &[&str], remote: bool, batch: bool) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args);
    if remote {
        apply_noninteractive_env(&mut cmd, cwd);
    }
    if batch {
        for (k, v) in batch_env_pairs(&ssh_setup(cwd)) {
            cmd.env(k, v);
        }
    }

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

/// Clone `url` into `dest` at the point specified by `pin` (CLI-18), shallow
/// (DSC-118), then deepen it when the checked-out `mind.toml` declares
/// `[source] history = "full"` (DSC-120). For a clone that persists as a
/// melded source; a throwaway clone uses [`clone_shallow_at`].
pub fn clone_at(url: &str, dest: &Path, pin: &Pin) -> Result<()> {
    clone_shallow_at(url, dest, pin)?;
    deepen_if_declared(url, dest)
}

/// Clone `url` into `dest` at `pin` at depth 1 (DSC-118), ignoring any
/// `[source] history` declaration. For throwaway clones (`review`, the TUI
/// preview) that only read the checked-out tree.
///
/// - `DefaultBranch` — shallow clone of the remote default branch.
/// - `FollowBranch(b)` — shallow clone with `--branch <b>`.
/// - `Tag(t)` — shallow clone with `--branch <t>` (git accepts tag names here).
/// - `Ref(sha)` — shallow clone of the default branch, then a depth-1 fetch of
///   the commit itself (falling back to the full history when the remote
///   refuses a fetch by sha), then check it out.
pub fn clone_shallow_at(url: &str, dest: &Path, pin: &Pin) -> Result<()> {
    reject_pin_refspec(pin)?;
    let dest_str = dest.to_string_lossy().into_owned();
    match pin {
        Pin::DefaultBranch => {
            // spec: DSC-66 - insert `--` before the URL so a URL beginning
            // with `-` is not parsed as a git option.
            run(url, None, &["clone", "--depth", "1", "--", url, &dest_str])?;
        }
        Pin::FollowBranch(branch) => {
            // --branch consumes its next argument as a value, not a positional,
            // so it is already safe.  Validate anyway (DSC-66) so a bad value
            // is caught at parse time before reaching any subprocess.
            // `--` before the URL guards the URL positional (DSC-66).
            run(
                url,
                None,
                &[
                    "clone", "--depth", "1", "--branch", branch, "--", url, &dest_str,
                ],
            )?;
        }
        Pin::Tag(tag) => {
            // git clone accepts a tag name as the --branch argument (value, not
            // positional).  Validate (DSC-66) for the same reason as above.
            // `--` before the URL guards the URL positional (DSC-66).
            run(
                url,
                None,
                &[
                    "clone", "--depth", "1", "--branch", tag, "--", url, &dest_str,
                ],
            )?;
        }
        Pin::Ref(sha) => {
            // spec: DSC-118 -- clone the default branch's tip at depth 1, then
            // fetch the pinned commit by itself (`ensure_commit`), rather than
            // the whole history.
            //
            // `git checkout <commit>` does not accept `--` before the commit
            // argument (that form means "path operands follow"). Injection
            // safety is provided by `validate_ref_value` at parse time (DSC-66),
            // which rejects any value starting with `-` before it reaches here.
            // `--` before the URL guards the URL positional (DSC-66).
            run(url, None, &["clone", "--depth", "1", "--", url, &dest_str])?;
            let target = ensure_commit(url, dest, sha)?;
            run(url, Some(dest), &["checkout", &target])?;
        }
        Pin::Version(v) => {
            // The caller resolves the range to a release tag first (VER-40);
            // the checkout is then a tag clone.
            let tag = resolved_tag(url, v)?;
            run(
                url,
                None,
                &[
                    "clone", "--depth", "1", "--branch", tag, "--", url, &dest_str,
                ],
            )?;
        }
    }
    Ok(())
}

/// The release tag a version pin resolved to. Unresolved is a caller bug, but
/// it surfaces as an error rather than a panic.
fn resolved_tag<'a>(url: &str, v: &'a crate::source::VersionPin) -> Result<&'a str> {
    v.tag
        .as_deref()
        .ok_or_else(|| MindError::NoMatchingRelease {
            source_name: url.to_string(),
            range: v.range.clone(),
            highest: v.highest.clone(),
        })
}

/// Clone `url` into `dest` at the default branch, the same as `clone_at` with
/// `Pin::DefaultBranch` (so a `history = "full"` declaration is honored).
pub fn clone(url: &str, dest: &Path) -> Result<()> {
    clone_at(url, dest, &Pin::DefaultBranch)
}

/// Whether the clone at `dir` is shallow (`rev-parse
/// --is-shallow-repository`). A failed check reads as shallow, which keeps
/// the fetch at depth 1 (the default) rather than pulling the full history.
fn is_shallow(url: &str, dir: &Path) -> bool {
    run(url, Some(dir), &["rev-parse", "--is-shallow-repository"]).map_or(true, |s| s != "false")
}

/// The depth arguments for a fetch into the clone at `dir` (DSC-119):
/// `--depth 1` while the clone is shallow, none once it holds the complete
/// history, so a fetch never truncates history the clone already has.
// spec: DSC-119
fn fetch_depth(url: &str, dir: &Path) -> &'static [&'static str] {
    if is_shallow(url, dir) {
        &["--depth", "1"]
    } else {
        &[]
    }
}

/// `git fetch <depth> <rest>` in `dir`, with the depth from [`fetch_depth`].
fn fetch(url: &str, dir: &Path, rest: &[&str]) -> Result<String> {
    let mut args: Vec<&str> = vec!["fetch"];
    args.extend_from_slice(fetch_depth(url, dir));
    args.extend_from_slice(rest);
    run(url, Some(dir), &args)
}

/// Make sure the commit `rev` names is in the clone at `dir` (DSC-118,
/// DSC-119), returning the revision to check out. `rev` is usually a sha, but
/// a `--pin <ref>` freeze (CLI-200) resolves a branch or tag name through the
/// same path. A commit already present is not fetched (a sha is immutable).
/// Otherwise it is fetched by itself; a remote that refuses that fetch gets a
/// full-history fetch of every branch and tag instead ([`fetch_all_history`]).
///
/// A name is fetched only as `refs/tags/<name>` or `refs/heads/<name>`
/// ([`pin_refs`]), never through git's own name lookup, and a hex prefix
/// resolves only to a commit, never to a ref spelled the same (DSC-122). The
/// result is always a full sha.
// spec: DSC-118 DSC-119 DSC-122
fn ensure_commit(url: &str, dir: &Path, rev: &str) -> Result<String> {
    if is_sha_prefix(rev) {
        if let Some(sha) = commit_with_prefix(url, dir, rev)? {
            return Ok(sha);
        }
        fetch_all_history(url, dir)?;
        return commit_with_prefix(url, dir, rev)?.ok_or_else(|| MindError::InvalidRef {
            value: rev.to_string(),
            reason: format!(
                "no commit on the remote's branches or tags starts with it (to pin a \
                 branch or tag with this name, use refs/heads/{rev} or refs/tags/{rev})"
            ),
        });
    }
    let object = format!("{rev}^{{commit}}");
    if is_full_sha(rev) {
        if run(url, Some(dir), &["cat-file", "-e", &object]).is_err() {
            if fetch(url, dir, &["origin", "--", rev]).is_err() {
                fetch_all_history(url, dir)?;
            }
            run(url, Some(dir), &["cat-file", "-e", &object])?;
        }
        return Ok(rev.to_string());
    }
    let refs = pin_refs(rev, false)?;
    if run(url, Some(dir), &["cat-file", "-e", &object]).is_ok() {
        return run(url, Some(dir), &["rev-parse", "--verify", &object]);
    }
    for remote_ref in &refs {
        if fetch(url, dir, &["origin", "--", remote_ref]).is_ok() {
            return run(url, Some(dir), &["rev-parse", "FETCH_HEAD^{commit}"]);
        }
    }
    // Not a plain branch or tag (e.g. `main~1`): resolve it against the
    // remote's branches and tags, the only refs the fallback fetches.
    fetch_all_history(url, dir)?;
    run(url, Some(dir), &["rev-parse", "--verify", &object])
}

/// Fetch the complete history of every branch and tag into the clone at `dir`
/// (`--unshallow` when shallow). The explicit refspecs matter: a depth-1 clone
/// is single-branch, so a plain `fetch origin` would miss a commit that only
/// another branch or a tag reaches.
fn fetch_all_history(url: &str, dir: &Path) -> Result<()> {
    let mut args: Vec<&str> = vec!["fetch"];
    if is_shallow(url, dir) {
        args.push("--unshallow");
    }
    args.extend_from_slice(&[
        "origin",
        "--",
        "+refs/heads/*:refs/remotes/origin/*",
        "+refs/tags/*:refs/tags/*",
    ]);
    run(url, Some(dir), &args).map(|_| ())
}

/// Whether `rev` is a full commit sha (40 hex digits, or 64 for SHA-256).
fn is_full_sha(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `rev` is an abbreviated commit sha: 4 (git's minimum) to 39 hex
/// digits (DSC-122).
fn is_sha_prefix(rev: &str) -> bool {
    (4..40).contains(&rev.len()) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The one commit in the clone at `dir` whose sha starts with `prefix`,
/// looked up among objects only (`rev-parse --disambiguate`), so a ref named
/// like the prefix never stands in for it (DSC-122). `None` when no commit
/// matches; an error when more than one does.
fn commit_with_prefix(url: &str, dir: &Path, prefix: &str) -> Result<Option<String>> {
    let arg = format!("--disambiguate={prefix}");
    let objects = run(url, Some(dir), &["rev-parse", &arg]).unwrap_or_default();
    let commits: Vec<&str> = objects
        .lines()
        .filter(|o| run(url, Some(dir), &["cat-file", "-t", o]).is_ok_and(|t| t == "commit"))
        .collect();
    match commits.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).to_string())),
        _ => Err(MindError::InvalidRef {
            value: prefix.to_string(),
            reason: "more than one commit starts with it; pin the full sha".to_string(),
        }),
    }
}

/// The remote refs a pin's name may be fetched as (DSC-122): a branch
/// (`refs/heads/<name>`) or a tag (`refs/tags/<name>`), never another
/// namespace such as GitHub's `refs/pull/<n>/head`. A `ref` pin tries the tag
/// first, the order git's own lookup uses; a `follow-branch` pin asks only for
/// the branch. A value already spelled `refs/heads/...` or `refs/tags/...` is
/// used as given. Any other `refs/...` value, or one holding refspec syntax
/// (`:`, `*`), is an `InvalidRef` error.
// spec: DSC-122
fn pin_refs(name: &str, branch_only: bool) -> Result<Vec<String>> {
    let invalid = |reason: &str| MindError::InvalidRef {
        value: name.to_string(),
        reason: reason.to_string(),
    };
    reject_refspec_syntax(name)?;
    if name.starts_with("refs/heads/") || name.starts_with("refs/tags/") {
        return Ok(vec![name.to_string()]);
    }
    if name.starts_with("refs/") {
        return Err(invalid(
            "a pin may name only refs/heads/<branch> or refs/tags/<tag>",
        ));
    }
    Ok(if branch_only {
        vec![format!("refs/heads/{name}")]
    } else {
        vec![format!("refs/tags/{name}"), format!("refs/heads/{name}")]
    })
}

/// [`reject_refspec_syntax`] for the value of a branch, tag, or `ref` pin, run
/// before a clone or sync touches the remote, so a refused value leaves no
/// partial clone behind.
fn reject_pin_refspec(pin: &Pin) -> Result<()> {
    match pin {
        Pin::FollowBranch(name) | Pin::Tag(name) | Pin::Ref(name) => reject_refspec_syntax(name),
        Pin::DefaultBranch | Pin::Version(_) => Ok(()),
    }
}

/// Refuse a pin value holding refspec syntax (DSC-122): `:` would turn a
/// fetch into `src:dst` and `*` into a pattern.
// spec: DSC-122
fn reject_refspec_syntax(name: &str) -> Result<()> {
    if name.contains([':', '*']) {
        return Err(MindError::InvalidRef {
            value: name.to_string(),
            reason: "a pin names a branch, tag, or commit; ':' and '*' are refspec syntax"
                .to_string(),
        });
    }
    Ok(())
}

/// Whether the tree checked out at `dir` declares `[source] history = "full"`
/// (DSC-120). An unreadable or invalid `mind.toml` reads as not declaring it:
/// the scan that follows reports the file's error.
fn declares_full_history(dir: &Path) -> bool {
    matches!(
        crate::mindfile::MindToml::load(dir),
        Ok(Some(t)) if t.source.history == Some(crate::mindfile::History::Full)
    )
}

/// Deepen the clone at `dir` to its complete history when its checked-out
/// `mind.toml` declares `history = "full"` and the clone is still shallow
/// (DSC-120). Once deepened, later fetches keep it complete (DSC-119).
// spec: DSC-120
fn deepen_if_declared(url: &str, dir: &Path) -> Result<()> {
    if declares_full_history(dir) {
        deepen(url, dir)?;
    }
    Ok(())
}

/// Deepen the clone at `dir` to its complete history (`fetch --unshallow`); a
/// no-op on a clone that is already complete. Used for the source's own
/// declaration (DSC-120) and the consumer's `meld --full-history` (DSC-121).
// spec: DSC-120 DSC-121
pub fn deepen(url: &str, dir: &Path) -> Result<()> {
    if is_shallow(url, dir) {
        run(url, Some(dir), &["fetch", "--unshallow", "origin"])?;
    }
    Ok(())
}

/// Resolve an existing clone against `pin` (CLI-55):
///
/// - `DefaultBranch` — fetch origin's default branch, reset to it.
/// - `FollowBranch(b)` — fetch that branch, reset to it.
/// - `Tag(t)` — force-fetch tags so a re-pointed tag is picked up, then reset
///   to the tag.
/// - `Ref(sha)` — fetch the pinned sha only when the clone does not already
///   hold it (DSC-119), then reset to the sha.  The recorded commit stays at
///   `sha` unless the caller changes the pin.
///
/// Fetches are depth 1 while the clone is shallow and full once it is
/// complete (DSC-119); afterwards a `history = "full"` declaration in the
/// checked-out tree deepens a shallow clone (DSC-120).
pub fn sync_to_pin(url: &str, dir: &Path, pin: &Pin) -> Result<()> {
    sync_to_pin_shallow(url, dir, pin)?;
    deepen_if_declared(url, dir)
}

fn sync_to_pin_shallow(url: &str, dir: &Path, pin: &Pin) -> Result<()> {
    reject_pin_refspec(pin)?;
    match pin {
        Pin::DefaultBranch => {
            fetch(url, dir, &["origin"])?;
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
            // end-of-options form for `git fetch`. The branch is fetched by
            // its qualified name so it cannot match another namespace (DSC-122).
            let remote_ref = pin_refs(branch, true)?.remove(0);
            fetch(url, dir, &["origin", "--", &remote_ref])?;
            // FETCH_HEAD is an internal git ref, not user-supplied.
            run(url, Some(dir), &["reset", "--hard", "FETCH_HEAD"])?;
        }
        Pin::Tag(tag) => {
            // Force-fetch tags so a re-pointed tag moves the local ref.
            // The refspec `+refs/tags/{tag}:refs/tags/{tag}` already encodes the
            // tag name inside a fixed-prefix string; the `--` before the refspec
            // guards the positional argument position (DSC-66). Depth 1 while
            // the clone is shallow (DSC-119). `reject_pin_refspec` above has
            // refused a `*` (a pattern) or `:` (a second destination).
            fetch(
                url,
                dir,
                &[
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
            // spec: DSC-119 -- a sha is immutable, so a clone that already
            // holds it needs no fetch; otherwise fetch just that commit.
            let target = ensure_commit(url, dir, sha)?;
            // Same as Tag: no `--` before the commit; injection safety comes
            // from `validate_ref_value` at parse time (DSC-66).
            run(url, Some(dir), &["reset", "--hard", &target])?;
        }
        Pin::Version(v) => {
            // The caller has already resolved the range and decided whether a
            // moved tag may be followed (VER-44); this only moves to the tag.
            let tag = resolved_tag(url, v)?;
            fetch(
                url,
                dir,
                &[
                    "--force",
                    "origin",
                    "--",
                    &format!("+refs/tags/{tag}:refs/tags/{tag}"),
                ],
            )?;
            run(url, Some(dir), &["reset", "--hard", tag])?;
        }
    }
    Ok(())
}

/// List the remote's tags as `(tag, commit)` pairs (VER-40). The shallow
/// clones `mind` makes carry no tag list, so this asks the remote directly. An
/// annotated tag is reported at the commit it peels to (`<tag>^{}`).
// spec: VER-40
pub fn ls_remote_tags(url: &str) -> Result<Vec<(String, String)>> {
    let out = run(url, None, &["ls-remote", "--tags", "--", url])?;
    let mut tags: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let Some((sha, name)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = name.strip_prefix("refs/tags/") else {
            continue;
        };
        match name.strip_suffix("^{}") {
            // The peeled line follows its tag object's line; it wins.
            Some(tag) => match tags.iter_mut().find(|(t, _)| t == tag) {
                Some(entry) => entry.1 = sha.to_string(),
                None => tags.push((tag.to_string(), sha.to_string())),
            },
            None => tags.push((name.to_string(), sha.to_string())),
        }
    }
    Ok(tags)
}

/// The text of `mind.toml` at the remote default branch's tip, fetched into
/// the clone at `dir` without moving its checkout (VER-3). `None` when the tip
/// has no `mind.toml`. Size-capped like every other metadata read (DSC-91).
// spec: VER-3
pub fn default_branch_mind_toml(url: &str, dir: &Path) -> Result<Option<String>> {
    fetch(url, dir, &["origin", "HEAD"])?;
    let Ok(size) = run(url, Some(dir), &["cat-file", "-s", "FETCH_HEAD:mind.toml"]) else {
        return Ok(None);
    };
    let limit = crate::error::metadata_size_limit();
    if size.parse::<u64>().map_or(true, |n| n > limit) {
        return Err(MindError::MetadataTooLarge {
            path: Path::new(url).join("mind.toml"),
            limit,
        });
    }
    run(
        url,
        Some(dir),
        &["cat-file", "blob", "FETCH_HEAD:mind.toml"],
    )
    .map(Some)
}

/// The message of release tag `tag`, fetched into the clone at `dir` without
/// moving its checkout (VER-64). `None` for a lightweight tag. The signature
/// block of a signed tag is left out, and the message is size-capped like
/// other metadata reads (DSC-91). The caller sanitizes it before printing.
// spec: VER-64
pub fn tag_annotation(url: &str, dir: &Path, tag: &str) -> Result<Option<String>> {
    validate_ref_value(tag)?;
    let tag_ref = format!("refs/tags/{tag}");
    fetch(
        url,
        dir,
        &["--force", "origin", "--", &format!("+{tag_ref}:{tag_ref}")],
    )?;
    if run(url, Some(dir), &["cat-file", "-t", &tag_ref])? != "tag" {
        return Ok(None);
    }
    let limit = crate::error::metadata_size_limit();
    let size = run(url, Some(dir), &["cat-file", "-s", &tag_ref])?;
    if size.parse::<u64>().map_or(true, |n| n > limit) {
        return Err(MindError::MetadataTooLarge {
            path: Path::new(url).join(&tag_ref),
            limit,
        });
    }
    let text = run(
        url,
        Some(dir),
        &[
            "for-each-ref",
            "--format=%(contents:subject)%0a%0a%(contents:body)",
            &tag_ref,
        ],
    )?;
    Ok((!text.is_empty()).then_some(text))
}

/// The text of `mind.toml` at release tag `tag` (VER-41), or `None` when that
/// release has none. A clone (`fetch`) gets the tag from origin first, without
/// moving its checkout; a local working tree already holds it and is left
/// untouched. Size-capped like other metadata reads (DSC-91).
// spec: VER-41
pub fn tag_mind_toml(url: &str, dir: &Path, tag: &str, fetch: bool) -> Result<Option<String>> {
    validate_ref_value(tag)?;
    let tag_ref = format!("refs/tags/{tag}");
    if fetch {
        self::fetch(
            url,
            dir,
            &["--force", "origin", "--", &format!("+{tag_ref}:{tag_ref}")],
        )?;
    }
    let spec = format!("{tag_ref}:mind.toml");
    let Ok(size) = run(url, Some(dir), &["cat-file", "-s", &spec]) else {
        return Ok(None);
    };
    let limit = crate::error::metadata_size_limit();
    if size.parse::<u64>().map_or(true, |n| n > limit) {
        return Err(MindError::MetadataTooLarge {
            path: Path::new(url).join(&spec),
            limit,
        });
    }
    run(url, Some(dir), &["cat-file", "blob", &spec]).map(Some)
}

/// Resolve the current HEAD commit sha of a clone.
pub fn head_commit(url: &str, dir: &Path) -> Result<String> {
    run(url, Some(dir), &["rev-parse", "HEAD"])
}

/// Put a clone back on `sha`, a commit it was on before a move. The object is
/// already local, so nothing is fetched, and a tag moved upstream since cannot
/// redirect it.
pub fn reset_to_commit(url: &str, dir: &Path, sha: &str) -> Result<()> {
    // `reset --hard` takes no `--` before a commit; the value comes from the
    // registry, so reject anything shaped like an option (DSC-66).
    validate_ref_value(sha)?;
    run(url, Some(dir), &["reset", "--hard", sha]).map(|_| ())
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

/// Whether git tracks `file` in the repo at `repo`, i.e. whether the file would
/// appear in a fresh clone. Used by `review` to catch tooling that resolves in
/// the author's working tree but is gitignored/unadded and so ships broken
/// (CLI-190). Returns `true` (treat as shippable, do not flag) when `repo` is not
/// a git repository, git is unavailable, or the paths cannot be resolved, so the
/// check only ever fires on a real, assessable repo.
pub fn is_tracked(repo: &Path, file: &Path) -> bool {
    if !is_repo(repo) {
        return true;
    }
    // Resolve both to absolute and take the repo-relative path as the pathspec,
    // so a relative `repo`/`file` (e.g. `mind review ./src`) is not misread
    // relative to git's own working directory. `file` must exist on disk (callers
    // only ask about present files), so canonicalize succeeds.
    let (Ok(repo_abs), Ok(file_abs)) = (repo.canonicalize(), file.canonicalize()) else {
        return true;
    };
    let Ok(rel) = file_abs.strip_prefix(&repo_abs) else {
        return true;
    };
    // `ls-files --error-unmatch` exits 0 iff the path is tracked.
    run(
        &repo_abs.to_string_lossy(),
        Some(&repo_abs),
        &["ls-files", "--error-unmatch", "--", &rel.to_string_lossy()],
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
        let pairs = noninteractive_env_pairs(&openssh("ssh"));
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
        let custom = noninteractive_env_pairs(&openssh("ssh -i /my/key"));
        let ssh2 = &custom[1].1;
        assert!(
            ssh2.starts_with("ssh -i /my/key ") && ssh2.contains("BatchMode=yes"),
            "custom base ssh command must be preserved and wrapped: {ssh2}"
        );
    }

    // spec: PRG-6 TUI-45
    #[test]
    fn the_ssh_base_follows_gits_own_precedence() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            resolve_ssh_base(s("ssh -v"), || s("ssh -i cfg"), s("/bin/plink")),
            "ssh -v",
            "GIT_SSH_COMMAND wins"
        );
        assert_eq!(
            resolve_ssh_base(None, || s("ssh -i cfg"), s("/bin/plink")),
            "ssh -i cfg",
            "core.sshCommand beats GIT_SSH"
        );
        assert_eq!(
            resolve_ssh_base(s(" "), || None, s("/opt/my ssh/it's")),
            r"'/opt/my ssh/it'\''s'",
            "an empty command is unset; GIT_SSH is a program, quoted"
        );
        assert_eq!(resolve_ssh_base(None, || s(""), None), "ssh");
    }

    /// An ssh setup with no explicit variant.
    fn openssh(command: &str) -> SshSetup {
        SshSetup {
            command: command.to_string(),
            variant: None,
        }
    }

    // spec: PRG-6 TUI-45
    #[test]
    fn prompts_go_off_only_in_a_form_the_ssh_program_accepts() {
        let opts = "-o BatchMode=yes";
        let with = |command: &str, variant: Option<&str>| {
            prompt_free_ssh(
                &SshSetup {
                    command: command.to_string(),
                    variant: variant.map(str::to_string),
                },
                opts,
            )
        };
        assert_eq!(
            with("/usr/bin/ssh -i k", None).as_deref(),
            Some("/usr/bin/ssh -i k -o BatchMode=yes")
        );
        assert_eq!(
            with(r#""C:\Program Files\PuTTY\PLINK.EXE""#, None).as_deref(),
            Some(r#""C:\Program Files\PuTTY\PLINK.EXE" -batch"#),
            "plink takes -batch, not -o"
        );
        assert_eq!(
            with("'/opt/putty' -v", None).as_deref(),
            Some("'/opt/putty' -v -batch")
        );
        assert_eq!(
            with("tortoiseplink", None),
            None,
            "git already passes -batch"
        );
        assert_eq!(
            with("~/bin/my-ssh-wrapper", None),
            None,
            "an unknown program is left as configured"
        );
        assert_eq!(
            with("~/bin/my-ssh-wrapper", Some("ssh")).as_deref(),
            Some("~/bin/my-ssh-wrapper -o BatchMode=yes"),
            "an explicit variant wins"
        );
        assert_eq!(
            with("ssh", Some("simple")),
            None,
            "ssh.variant=simple takes no options"
        );
        assert_eq!(
            with("ssh", Some("auto")).as_deref(),
            Some("ssh -o BatchMode=yes")
        );

        // With no prompt-free form, the env still stops git's own prompts and
        // leaves the ssh command alone.
        let pairs = batch_env_pairs(&openssh("~/bin/my-ssh-wrapper"));
        assert_eq!(pairs, [("GIT_TERMINAL_PROMPT", "0".to_string())]);
    }

    // spec: PRG-6 TUI-45
    #[test]
    fn config_ssh_parses_the_last_value_of_each_key() {
        assert_eq!(
            parse_config_ssh(
                "core.sshcommand ssh -i /a\nssh.variant plink\ncore.sshcommand ssh -i /b\n"
            ),
            ConfigSsh {
                command: Some("ssh -i /b".to_string()),
                variant: Some("plink".to_string()),
            }
        );
        assert_eq!(parse_config_ssh(""), ConfigSsh::default());
    }

    // spec: PRG-6 TUI-45
    #[test]
    fn config_ssh_reads_the_repos_core_ssh_command() {
        let base = std::env::temp_dir().join(format!(
            "mind-git-sshcmd-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&base).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&base)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q"]);
        git(&["config", "core.sshCommand", "ssh -i /work/key"]);
        git(&["config", "ssh.variant", "ssh"]);
        assert_eq!(
            config_ssh(Some(&base)),
            ConfigSsh {
                command: Some("ssh -i /work/key".to_string()),
                variant: Some("ssh".to_string()),
            }
        );
        cleanup(&base);
    }

    #[test]
    fn with_noninteractive_applies_the_env_on_this_thread_only() {
        // spec: CLI-248 - a parallel fetch worker's git children never prompt;
        // the override is scoped to the closure and to the calling thread.
        let prompt_off = |cmd: &Command| {
            cmd.get_envs()
                .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == Some("0".as_ref()))
        };
        let mut inside = Command::new("git");
        with_noninteractive(|| apply_noninteractive_env(&mut inside, None));
        assert!(prompt_off(&inside), "worker git must not prompt");

        // The negative cases read the per-thread flag directly: another test
        // toggles the process-wide flag concurrently.
        let thread_flag = || THREAD_NONINTERACTIVE.with(|c| c.get());
        assert!(!thread_flag(), "override must end with the closure");
        let other = with_noninteractive(|| std::thread::spawn(thread_flag).join().unwrap());
        assert!(!other, "override must not leak to another thread");
    }

    // spec: PRG-6
    #[test]
    fn batch_env_disables_prompts_without_a_connect_timeout() {
        let pairs = batch_env_pairs(&openssh("ssh -i /my/key"));
        assert_eq!(
            pairs,
            [
                ("GIT_TERMINAL_PROMPT", "0".to_string()),
                (
                    "GIT_SSH_COMMAND",
                    "ssh -i /my/key -o BatchMode=yes".to_string()
                ),
            ]
        );
    }

    // spec: PRG-6
    #[test]
    fn prompt_retry_reruns_a_suppressed_prompt_with_prompts_on() {
        let mut calls = Vec::new();
        let r = with_prompt_retry(true, |batch| {
            calls.push(batch);
            if batch {
                Err(git_err(
                    "fatal: could not read Username for 'https://x': terminal prompts disabled",
                ))
            } else {
                Ok("done")
            }
        });
        assert_eq!(r.unwrap(), "done");
        assert_eq!(calls, [true, false]);
    }

    // spec: PRG-6
    #[test]
    fn prompt_retry_leaves_other_failures_and_non_batch_runs_alone() {
        let mut calls = Vec::new();
        let r: Result<()> = with_prompt_retry(true, |batch| {
            calls.push(batch);
            Err(git_err("fatal: repository 'x' not found"))
        });
        assert!(r.is_err());
        assert_eq!(
            calls,
            [true],
            "a failure that is not a prompt is not retried"
        );

        let mut calls = Vec::new();
        let r: Result<()> = with_prompt_retry(false, |batch| {
            calls.push(batch);
            Err(git_err("fatal: Authentication failed"))
        });
        assert!(r.is_err());
        assert_eq!(calls, [false], "a run that already prompted is not retried");
    }

    // spec: PRG-4
    #[test]
    fn network_subcommands_get_a_label_local_ones_do_not() {
        let url = "https://example.com/o/r";
        assert_eq!(
            network_label(url, &["clone", "--", url, "/d"]).as_deref(),
            Some("cloning https://example.com/o/r")
        );
        assert_eq!(
            network_label(url, &["fetch", "origin"]).as_deref(),
            Some("fetching https://example.com/o/r")
        );
        assert_eq!(
            network_label(url, &["ls-remote", "--tags", "--", url]).as_deref(),
            Some("listing tags of https://example.com/o/r")
        );
        assert_eq!(network_label(url, &["rev-parse", "HEAD"]), None);
        assert_eq!(network_label(url, &[]), None);
    }

    #[test]
    fn suppressed_prompt_failures_cover_credentials_and_host_keys() {
        // spec: CLI-248 - the failures a non-interactive git reports in place
        // of a prompt are the ones a parallel sync retries interactively.
        let git_err = |stderr: &str| MindError::Git {
            url: "u".into(),
            args: vec!["fetch".into()],
            status: None,
            stderr: stderr.into(),
        };
        assert!(is_suppressed_prompt_failure(&git_err(
            "fatal: could not read Username for 'https://x': terminal prompts disabled"
        )));
        assert!(is_suppressed_prompt_failure(&git_err(
            "Host key verification failed.\nfatal: Could not read from remote repository."
        )));
        assert!(is_suppressed_prompt_failure(&git_err(
            "git@x: Permission denied (publickey)."
        )));
        assert!(!is_suppressed_prompt_failure(&git_err(
            "fatal: unable to access 'https://x/': Could not resolve host: x"
        )));
        assert!(!is_suppressed_prompt_failure(&MindError::GitNotFound));
    }

    // spec: TUI-45
    #[test]
    fn only_the_tui_detaches_and_only_an_ssh_it_cannot_quiet() {
        let wrapper = openssh("~/bin/my-ssh-wrapper");
        assert!(must_detach(&wrapper, true));
        assert!(!must_detach(&wrapper, false), "a fetch worker keeps Ctrl-C");
        assert!(
            !must_detach(&openssh("ssh -i k"), true),
            "BatchMode is enough"
        );
        assert!(!must_detach(&openssh("plink"), true), "-batch is enough");
    }

    // spec: TUI-45
    #[cfg(unix)]
    #[test]
    fn a_detached_git_child_has_no_controlling_terminal() {
        // `setsid` makes the child lead a new session, with no controlling
        // terminal, and a new process group whose id is its own pid. The
        // group is read with `getpgid`, which works the same on Linux and
        // macOS (`ps -o sid=` does not).
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        detach_from_terminal(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        // SAFETY: plain queries of a live child and of this process.
        let (group, ours) = unsafe { (libc::getpgid(pid), libc::getpgid(0)) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(group, pid, "the child leads its own process group");
        assert_ne!(group, ours);
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
    fn ls_remote_tags_peels_annotated_tags() {
        // The tag listing VER-40 resolves against.
        let base = tmpdir("ls-remote-tags");
        let (remote, sha_a, sha_b, _c) = make_remote(&base);
        git(&remote, &["tag", "-a", "v2.0.0", "-m", "notes", &sha_b]);
        let url = format!("file://{}", remote.display());
        let mut tags = ls_remote_tags(&url).unwrap();
        tags.sort();
        assert_eq!(
            tags,
            [
                ("v1.0".to_string(), sha_a.clone()),
                ("v2.0.0".to_string(), sha_b.clone())
            ]
        );
        cleanup(&base);
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

    // ---- clone depth (DSC-118..120) -----------------------------------------

    /// Whether `dir` holds commit `sha`.
    fn has_commit(dir: &Path, sha: &str) -> bool {
        Command::new("git")
            .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .current_dir(dir)
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Commit a root `mind.toml` declaring `history = "full"` on the remote.
    fn declare_full_history(remote: &Path) -> String {
        fs::write(remote.join("mind.toml"), "[source]\nhistory = \"full\"\n").unwrap();
        git(remote, &["add", "mind.toml"]);
        git(remote, &["commit", "-qm", "declare full history"]);
        read_head(remote)
    }

    #[test]
    fn clone_at_every_pin_is_shallow() {
        // spec: DSC-118 -- every pin kind clones at depth 1.
        let base = tmpdir("depth-all");
        let (remote, sha_a, sha_b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let pins = [
            ("default", Pin::DefaultBranch),
            ("follow", Pin::FollowBranch("stable".into())),
            ("tag", Pin::Tag("v1.0".into())),
            ("ref", Pin::Ref(sha_b.clone())),
        ];
        for (name, pin) in pins {
            let dest = base.join(name);
            clone_at(&url, &dest, &pin).unwrap();
            assert!(is_shallow(&url, &dest), "{name} clone must be shallow");
        }
        // The ref pin fetched B by itself: its parent A never arrived.
        assert!(has_commit(&base.join("ref"), &sha_b));
        assert!(
            !has_commit(&base.join("ref"), &sha_a),
            "a ref pin must fetch only the pinned commit, not its history"
        );
        cleanup(&base);
    }

    /// Point `refs/pull/1/head` on `remote` at a new commit no branch or tag
    /// reaches (GitHub's pull request heads), and return that commit.
    fn add_pull_head(remote: &Path) -> String {
        git(remote, &["checkout", "-q", "--detach"]);
        fs::write(remote.join("file.txt"), "pull request").unwrap();
        git(remote, &["commit", "-aqm", "pull request"]);
        let pr = read_head(remote);
        git(remote, &["update-ref", "refs/pull/1/head", &pr]);
        git(remote, &["checkout", "-q", "main"]);
        pr
    }

    #[test]
    fn ref_pin_name_never_fetches_outside_branches_and_tags() {
        // spec: DSC-122 -- git's own name lookup would match `pull/1/head` to
        // the remote's `refs/pull/1/head`; a pin reaches only heads and tags.
        let base = tmpdir("pin-pull-ref");
        let (remote, _a, _b, _c) = make_remote(&base);
        let pr = add_pull_head(&remote);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        assert!(clone_at(&url, &dest, &Pin::Ref("pull/1/head".into())).is_err());
        if dest.exists() {
            assert!(!has_commit(&dest, &pr), "the pull request never arrives");
        }
        let err = clone_at(&url, &base.join("q"), &Pin::Ref("refs/pull/1/head".into()))
            .expect_err("a refs/pull value is refused");
        assert!(matches!(err, MindError::InvalidRef { .. }), "{err}");
        cleanup(&base);
    }

    #[test]
    fn follow_branch_sync_never_fetches_outside_branches() {
        // spec: DSC-122 -- a followed branch is fetched as refs/heads/<name>.
        let base = tmpdir("follow-pull-ref");
        let (remote, _a, _b, sha_c) = make_remote(&base);
        let pr = add_pull_head(&remote);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert!(sync_to_pin(&url, &dest, &Pin::FollowBranch("pull/1/head".into())).is_err());
        assert!(!has_commit(&dest, &pr));
        assert_eq!(read_head(&dest), sha_c, "a failed sync leaves the clone");
        // A qualified branch name still follows.
        sync_to_pin(&url, &dest, &Pin::FollowBranch("refs/heads/stable".into())).unwrap();
        assert_eq!(read_file(&dest, "file.txt"), "version A");
        cleanup(&base);
    }

    #[test]
    fn short_sha_pin_resolves_to_the_commit_not_a_same_named_ref() {
        // spec: DSC-122 -- a branch named like the pinned prefix must not
        // stand in for the commit, on a first clone or on a sync.
        let base = tmpdir("pin-short-sha");
        let (remote, _a, sha_b, _c) = make_remote(&base);
        let short = &sha_b[..8];
        git(&remote, &["checkout", "-q", "--orphan", "decoy"]);
        fs::write(remote.join("file.txt"), "decoy").unwrap();
        git(&remote, &["commit", "-aqm", "decoy"]);
        git(&remote, &["branch", "-m", short]);
        git(&remote, &["tag", short]);
        git(&remote, &["checkout", "-q", "main"]);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(short.to_string())).unwrap();
        assert_eq!(read_head(&dest), sha_b);
        sync_to_pin(&url, &dest, &Pin::Ref(short.to_string())).unwrap();
        assert_eq!(read_head(&dest), sha_b);

        let missing = base.join("missing");
        let err = clone_at(&url, &missing, &Pin::Ref("0000000f".into()))
            .expect_err("a prefix no commit has fails");
        assert!(matches!(err, MindError::InvalidRef { .. }), "{err}");
        assert!(
            err.to_string().contains("refs/tags/0000000f"),
            "the error names the qualified spelling: {err}"
        );
        // A same-named tag is reachable by its qualified name.
        let qualified = base.join("qualified");
        clone_at(&url, &qualified, &Pin::Ref(format!("refs/tags/{short}"))).unwrap();
        assert_ne!(read_head(&qualified), sha_b, "the tag, not the commit");
        cleanup(&base);
    }

    #[test]
    fn pin_names_holding_refspec_syntax_are_refused() {
        // spec: DSC-122 -- `:` would turn the fetch into src:dst and write a
        // local ref; `*` would fetch a pattern.
        let base = tmpdir("pin-refspec");
        let (remote, _a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        let mut fresh = 0;
        for bad in ["stable:refs/heads/planted", "refs/heads/*", "v*"] {
            for pin in [
                Pin::Ref(bad.into()),
                Pin::FollowBranch(bad.into()),
                Pin::Tag(bad.into()),
            ] {
                let err = sync_to_pin(&url, &dest, &pin).expect_err(bad);
                assert!(matches!(err, MindError::InvalidRef { .. }), "{bad}: {err}");
                fresh += 1;
                let target = base.join(format!("fresh{fresh}"));
                let err = clone_at(&url, &target, &pin).expect_err(bad);
                assert!(matches!(err, MindError::InvalidRef { .. }), "{bad}: {err}");
                assert!(!target.exists(), "{bad}: refused before cloning");
            }
        }
        let tag = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", "refs/tags/v1.0"])
            .current_dir(&dest)
            .status()
            .unwrap();
        assert!(!tag.success(), "the `v*` tag pin fetched no tags");
        let planted = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", "refs/heads/planted"])
            .current_dir(&dest)
            .status()
            .unwrap();
        assert!(!planted.success(), "no local ref was written");
        cleanup(&base);
    }

    #[test]
    fn ref_pin_by_tag_or_branch_name_resolves_to_its_commit() {
        // spec: DSC-122 -- plain and qualified tag and branch names still
        // resolve, a tag before a branch of the same name.
        let base = tmpdir("pin-names");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        for (i, name) in ["v1.0", "refs/tags/v1.0", "stable", "refs/heads/stable"]
            .iter()
            .enumerate()
        {
            let dest = base.join(format!("c{i}"));
            clone_at(&url, &dest, &Pin::Ref((*name).into())).unwrap();
            assert_eq!(read_head(&dest), sha_a, "{name}");
        }
        cleanup(&base);
    }

    #[test]
    fn ref_pin_falls_back_to_full_fetch_when_sha_fetch_is_refused() {
        // spec: DSC-118 -- a remote that refuses a fetch by sha (protocol v0
        // with no allow*SHA1InWant) gets a full fetch of every branch and tag,
        // so a commit only another branch reaches is still found.
        let base = tmpdir("depth-fallback");
        let (remote, _a, _b, _c) = make_remote(&base);
        git(&remote, &["checkout", "-qb", "side"]);
        fs::write(remote.join("side.txt"), "side 1").unwrap();
        git(&remote, &["add", "side.txt"]);
        git(&remote, &["commit", "-qm", "side 1"]);
        let side_1 = read_head(&remote);
        fs::write(remote.join("side.txt"), "side 2").unwrap();
        git(&remote, &["commit", "-aqm", "side 2"]);
        git(&remote, &["checkout", "-q", "main"]);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_shallow_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        // Protocol v0 refuses a want for a commit no ref advertises.
        git(&dest, &["config", "protocol.version", "0"]);
        assert!(!has_commit(&dest, &side_1));
        ensure_commit(&url, &dest, &side_1).expect("fallback fetch finds the commit");
        assert!(has_commit(&dest, &side_1));
        assert!(!is_shallow(&url, &dest), "the fallback deepens the clone");
        cleanup(&base);
    }

    #[test]
    fn sync_ref_pin_holding_the_commit_does_not_fetch() {
        // spec: DSC-119 -- a sha is immutable: with the commit already local,
        // sync never reaches the remote (here, a remote that no longer exists).
        let base = tmpdir("depth-ref-nofetch");
        let (remote, _a, sha_b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::Ref(sha_b.clone())).unwrap();
        let gone = format!("file://{}", base.join("gone").display());
        git(&dest, &["remote", "set-url", "origin", &gone]);
        sync_to_pin(&gone, &dest, &Pin::Ref(sha_b.clone())).expect("no fetch needed");
        assert_eq!(read_head(&dest), sha_b);
        cleanup(&base);
    }

    #[test]
    fn complete_clone_stays_complete_across_syncs() {
        // spec: DSC-119 -- once a clone holds its whole history, fetches run
        // without --depth, so a sync (and a metadata read) never truncates it.
        let base = tmpdir("depth-stays-full");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_shallow_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        git(&dest, &["fetch", "-q", "--unshallow", "origin"]);
        assert!(!is_shallow(&url, &dest));

        fs::write(remote.join("file.txt"), "version D").unwrap();
        git(&remote, &["commit", "-aqm", "commit D"]);
        let sha_d = read_head(&remote);
        sync_to_pin(&url, &dest, &Pin::DefaultBranch).unwrap();
        default_branch_mind_toml(&url, &dest).unwrap();
        assert_eq!(read_head(&dest), sha_d);
        assert!(!is_shallow(&url, &dest), "a sync must not re-shallow");
        assert!(has_commit(&dest, &sha_a));
        cleanup(&base);
    }

    #[test]
    fn history_full_deepens_the_clone() {
        // spec: DSC-120 -- `[source] history = "full"` in the checked-out tree
        // deepens the clone; a throwaway clone ignores it.
        let base = tmpdir("depth-history-full");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        declare_full_history(&remote);
        let url = format!("file://{}", remote.display());

        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert!(!is_shallow(&url, &dest), "history = full must deepen");
        assert!(has_commit(&dest, &sha_a));

        let throwaway = base.join("throwaway");
        clone_shallow_at(&url, &throwaway, &Pin::DefaultBranch).unwrap();
        assert!(is_shallow(&url, &throwaway));
        cleanup(&base);
    }

    #[test]
    fn reset_to_commit_restores_a_shallow_clone_after_moving_forward() {
        // spec: DSC-119 AUP-17 -- auto-update's restore puts a clone back on
        // the commit it was on before a sync moved it. On a shallow clone the
        // old commit must still be local after the depth-1 fetch, with no
        // fetch of its own (the remote here is gone by the time it runs).
        let base = tmpdir("depth-restore");
        let (remote, _a, _b, sha_c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert!(is_shallow(&url, &dest));
        assert_eq!(read_head(&dest), sha_c);

        fs::write(remote.join("file.txt"), "version D").unwrap();
        git(&remote, &["commit", "-aqm", "commit D"]);
        let sha_d = read_head(&remote);
        sync_to_pin(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert_eq!(read_head(&dest), sha_d);
        assert!(is_shallow(&url, &dest));

        let gone = format!("file://{}", base.join("gone").display());
        git(&dest, &["remote", "set-url", "origin", &gone]);
        reset_to_commit(&gone, &dest, &sha_c).expect("old commit is still local");
        assert_eq!(read_head(&dest), sha_c);
        assert_eq!(read_file(&dest, "file.txt"), "version C");
        cleanup(&base);
    }

    #[test]
    fn deepen_is_a_no_op_on_a_complete_clone() {
        // spec: DSC-121 -- deepening twice (a re-meld with --full-history, or
        // a sync of an already-deepened clone) must not fail on the
        // `--unshallow on a complete repository` error.
        let base = tmpdir("depth-deepen-twice");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_shallow_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        deepen(&url, &dest).unwrap();
        assert!(!is_shallow(&url, &dest));
        assert!(has_commit(&dest, &sha_a));
        deepen(&url, &dest).expect("a second deepen is a no-op");
        cleanup(&base);
    }

    #[test]
    fn history_full_declared_later_deepens_on_sync() {
        // spec: DSC-120 -- a source that declares `history = "full"` after it
        // was melded is deepened by the next sync that checks the file out.
        let base = tmpdir("depth-history-sync");
        let (remote, sha_a, _b, _c) = make_remote(&base);
        let url = format!("file://{}", remote.display());
        let dest = base.join("clone");
        clone_at(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert!(is_shallow(&url, &dest));

        declare_full_history(&remote);
        sync_to_pin(&url, &dest, &Pin::DefaultBranch).unwrap();
        assert!(!is_shallow(&url, &dest));
        assert!(has_commit(&dest, &sha_a));
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
    fn is_auth_failure_matches_could_not_read_password() {
        // spec: DSC-68 -- URL-embedded-username path under GIT_TERMINAL_PROMPT=0
        let err = git_err(
            "fatal: could not read Password for 'https://user@github.com': terminal prompts disabled",
        );
        assert!(is_auth_failure(&err), "could not read Password must match");
    }

    #[test]
    fn is_auth_failure_matches_http_basic_access_denied() {
        // spec: DSC-68 -- GitLab credential failure
        let err = git_err(
            "remote: HTTP Basic: Access denied. The provided password or token is incorrect.",
        );
        assert!(
            is_auth_failure(&err),
            "HTTP Basic: Access denied must match"
        );
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

    // ---- CLI-178: is_proxy_failure detection --------------------------------

    #[test]
    fn is_proxy_failure_matches_received_http_code_407() {
        // spec: CLI-178 -- git HTTP backend 407 proxy response
        let err = git_err(
            "fatal: unable to access 'https://github.com/owner/repo.git/': \
             Received HTTP code 407 from proxy after CONNECT",
        );
        assert!(
            is_proxy_failure(&err),
            "Received HTTP code 407 from proxy must match"
        );
    }

    #[test]
    fn is_proxy_failure_matches_url_returned_407() {
        // spec: CLI-178 -- curl backend returning a 407 status
        let err = git_err(
            "fatal: unable to access 'https://github.com/owner/repo.git/': \
             The requested URL returned error: 407",
        );
        assert!(
            is_proxy_failure(&err),
            "The requested URL returned error: 407 must match"
        );
    }

    #[test]
    fn is_proxy_failure_matches_could_not_resolve_proxy() {
        // spec: CLI-178 -- DNS failure reaching the proxy host itself
        let err = git_err(
            "fatal: unable to access 'https://github.com/owner/repo.git/': \
             Could not resolve proxy: proxy.corp.example.com",
        );
        assert!(is_proxy_failure(&err), "Could not resolve proxy must match");
    }

    #[test]
    fn is_proxy_failure_does_not_match_auth_failure() {
        // spec: CLI-178 -- an auth failure is not a proxy failure
        let err = git_err("fatal: Authentication failed for 'https://github.com/owner/repo/'");
        assert!(
            !is_proxy_failure(&err),
            "auth failure must not match proxy failure"
        );
    }

    #[test]
    fn is_proxy_failure_does_not_match_network_error() {
        // spec: CLI-178 -- a plain network error is not a proxy failure
        let err = git_err("fatal: unable to connect to github.com: Connection refused");
        assert!(
            !is_proxy_failure(&err),
            "plain network error must not match proxy failure"
        );
    }

    #[test]
    fn is_proxy_failure_does_not_match_non_git_error() {
        // spec: CLI-178 -- a non-Git MindError is never a proxy failure
        let err = MindError::GitNotFound;
        assert!(
            !is_proxy_failure(&err),
            "GitNotFound must not be a proxy failure"
        );
    }

    #[test]
    fn is_proxy_failure_is_case_insensitive() {
        // spec: CLI-178 -- pattern matching is case-insensitive
        let err = git_err("RECEIVED HTTP CODE 407 FROM PROXY AFTER CONNECT");
        assert!(
            is_proxy_failure(&err),
            "proxy failure detection must be case-insensitive"
        );
    }

    // ---- CLI-177 / CLI-178: hint line content --------------------------------

    #[test]
    fn auth_hint_lines_include_ssh_url_and_config_and_helper() {
        // spec: CLI-177 -- the three remediation options are all present.
        let lines = auth_hint_lines("git@github.com:owner/private");
        let combined = lines.join("\n");
        assert!(
            combined.contains("git@github.com:owner/private"),
            "SSH URL must appear in auth hint: {combined}"
        );
        assert!(
            combined.contains("ssh = true"),
            "config option must appear in auth hint: {combined}"
        );
        assert!(
            combined.contains("credential helper"),
            "credential helper mention must appear in auth hint: {combined}"
        );
    }

    // spec: CLI-177 -- U33: GitHub answers an unauthenticated request for a
    // missing, private, or mistyped repo with the same credential prompt as a
    // real auth failure, so this note must come BEFORE the SSH/credential
    // remedies, not just appear somewhere in the hint.
    #[test]
    fn auth_hint_lines_lead_with_not_found_or_misspelled_note_before_remedies() {
        let lines = auth_hint_lines("git@github.com:owner/private");
        let combined = lines.join("\n");
        assert!(
            combined.contains("does not exist") && combined.contains("misspelled"),
            "must note the repo may not exist / be misspelled: {combined}"
        );
        assert!(
            combined.contains("private"),
            "must note the repo may be private: {combined}"
        );

        // Ordering: the not-found/misspelled note appears before the first SSH
        // remedy line, so a reader hits the cheaper check first.
        let note_pos = combined
            .find("does not exist")
            .expect("note must be present");
        let ssh_pos = combined
            .find("git@github.com:owner/private")
            .expect("SSH remedy must be present");
        assert!(
            note_pos < ssh_pos,
            "the not-found/misspelled note must precede the SSH remedy: {combined}"
        );

        // The original three remedies are all still present and unchanged.
        assert!(combined.contains("to use SSH"), "{combined}");
        assert!(combined.contains("ssh = true"), "{combined}");
        assert!(combined.contains("credential helper"), "{combined}");
    }

    #[test]
    fn proxy_hint_lines_mention_https_proxy_and_http_proxy_setting() {
        // spec: CLI-178 -- both HTTPS_PROXY and http.proxy are mentioned.
        let lines = proxy_hint_lines();
        let combined = lines.join("\n");
        assert!(
            combined.contains("HTTPS_PROXY"),
            "HTTPS_PROXY must appear in proxy hint: {combined}"
        );
        assert!(
            combined.contains("http.proxy"),
            "http.proxy must appear in proxy hint: {combined}"
        );
    }
}
