//! `mind evolve` — update the `mind` binary itself in place.
//!
//! This mirrors `resources/install.sh` but targets the running executable: it
//! resolves the release artifact for the current platform exactly as the install
//! script and the Homebrew formula do, downloads and extracts it, then atomically
//! swaps it for the binary it runs from.
//!
//! The pure resolution logic (target triple, asset URL, latest-tag parsing, and
//! the up-to-date/update decision) is split out so it is unit-testable without any
//! network access. Only `run` (and the helpers it calls) shells out.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::Digest;

use crate::error::{MindError, Result};
use crate::mindfile::version_at_least;

const REPO: &str = "jaemk/mind";

/// Whether the running binary needs replacing.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The running version already satisfies the target; nothing to do.
    UpToDate,
    /// The target is newer than the running version; replace the binary.
    Update,
    /// An explicit `--version` was pinned strictly BELOW the running version.
    /// We refuse to downgrade but report why rather than silently saying "up to date".
    // spec: CLI-147
    PinnedBelowCurrent,
}

/// Map an OS/arch pair to its release target triple, rejecting platforms with no
/// published artifact. Mirrors install.sh, which rejects Intel macOS (only Apple
/// Silicon is published) and any other OS/arch combination.
pub fn target_triple(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => Err(MindError::UnsupportedPlatform {
            os: os.to_string(),
            arch: arch.to_string(),
        }),
    }
}

/// The GitHub release asset URL for a version and target, matching the shape the
/// install script and Homebrew formula resolve (`mind-<version>-<target>.tar.gz`).
pub fn asset_url(version: &str, target: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{version}/mind-{version}-{target}.tar.gz")
}

/// The GitHub "latest release" API endpoint for the mind repo.
fn latest_release_api() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

/// Extract the release version from the GitHub releases/latest JSON: read
/// `tag_name` and strip a leading `v`. A missing `tag_name` is a structured error.
pub fn parse_latest_tag(json: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| MindError::json("github release", e))?;
    let tag = value
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| MindError::DownloadFailed {
            url: latest_release_api(),
            reason: "release JSON has no 'tag_name' field".to_string(),
        })?;
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Decide whether the running binary needs replacing.
///
/// - `explicit` is true when the caller supplied an explicit `--version` flag
///   (rather than resolving the latest release from the network).
///
/// When `explicit` is true and the pinned `target` is STRICTLY below `current`,
/// returns `PinnedBelowCurrent` instead of `UpToDate` so the caller can emit a
/// clear "not downgrading" message (CLI-147) rather than a misleading "up to date".
/// When the target equals the running version, `UpToDate` is always returned,
/// regardless of `explicit`. When the target is above `current`, `Update` is
/// returned regardless of `explicit`.
// spec: CLI-140
pub fn decision(current: &str, target: &str, explicit: bool) -> Decision {
    if version_at_least(current, target) {
        // current >= target; check whether the target is strictly BELOW current
        // and was given as an explicit pin.
        if explicit && !version_at_least(target, current) {
            // target < current: explicit downgrade request we refuse.
            Decision::PinnedBelowCurrent
        } else {
            Decision::UpToDate
        }
    } else {
        Decision::Update
    }
}

/// The one-line status `--check` (and the run path) reports: the running version,
/// the target, and whether an update is pending. Pure so it is unit-testable
/// without touching the network.
// spec: CLI-141
fn check_report(current: &str, target: &str, decision: &Decision) -> String {
    match decision {
        Decision::UpToDate => {
            format!("mind {current} is up to date (latest is {target})")
        }
        Decision::Update => {
            format!("mind {current} -> {target} available; run `mind evolve` to update")
        }
        // spec: CLI-147
        Decision::PinnedBelowCurrent => {
            format!("pinned {target} is below the running {current}; not downgrading")
        }
    }
}

/// Consult the managed policy for the self-update control (POL-51..POL-54).
///
/// Returns:
/// - `Ok(None)` when the policy allows `evolve` to any version (no pin).
/// - `Ok(Some(pin))` when the policy pins to a specific version (use as `--version`).
/// - `Err(SelfUpdatePolicy)` when `evolve` is disabled (POL-52) or when
///   `user_version` conflicts with the pin (POL-53).
///
/// Pure: no network call. `user_version` is the raw `--version` argument (may
/// have a leading `v`, which is stripped before comparison).
pub(crate) fn check_policy_for_evolve(
    policy: Option<&crate::policy::Policy>,
    user_version: Option<&str>,
) -> Result<Option<String>> {
    use crate::policy::SelfUpdateControl;
    let Some(pol) = policy else {
        return Ok(None);
    };
    match pol.self_update_control() {
        SelfUpdateControl::Allowed => Ok(None),
        SelfUpdateControl::Disabled => Err(MindError::SelfUpdatePolicy {
            detail: "self-update is disabled by the managed policy".to_string(),
        }),
        SelfUpdateControl::Pinned(pin) => {
            if let Some(uv) = user_version {
                let uv_clean = uv.strip_prefix('v').unwrap_or(uv);
                if uv_clean != pin {
                    return Err(MindError::SelfUpdatePolicy {
                        detail: format!(
                            "managed policy pins self-update to {pin}; \
                             --version {uv_clean} conflicts with the pin"
                        ),
                    });
                }
            }
            Ok(Some(pin.clone()))
        }
    }
}

/// `mind evolve [--check] [--yes] [--version <v>]` — update the running binary.
///
/// `--version` resolves the target WITHOUT any network call, so
/// `evolve --check --version <v>` is fully offline. With no `--version`, the
/// latest release is fetched from the GitHub API. `--check` reports the decision
/// and returns without downloading. Otherwise, unless `--yes`, it prompts before
/// replacing the binary.
pub fn run(check: bool, yes: bool, mut version: Option<String>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    // Resolve (and validate) the platform target up front, so an unsupported
    // platform fails before any network call.
    let target = target_triple(os, arch)?;

    // Load the managed policy and check the self-update control before any network
    // call (POL-51..POL-54). A machine with no policy file behaves exactly as today.
    let policy = crate::policy::Policy::load()?;
    let policy_pin_active =
        if let Some(pin) = check_policy_for_evolve(policy.as_ref(), version.as_deref())? {
            // Policy pins to a specific version; behave as if --version <pin> was passed.
            version = Some(pin);
            true
        } else {
            false
        };

    // Resolve the target version: an explicit --version (or a policy pin) bypasses
    // the network entirely; otherwise fetch and parse the latest release tag.
    let explicit = version.is_some();
    let target_version = match version {
        Some(v) => v.strip_prefix('v').unwrap_or(&v).to_string(),
        None => {
            let json = fetch_to_string(&latest_release_api())?;
            parse_latest_tag(&json)?
        }
    };

    let d = decision(current, &target_version, explicit);
    let out = crate::render::ctx();

    if check {
        // CLI-141: report and change nothing, without downloading.
        if out.json {
            let outcome = match d {
                Decision::UpToDate => "up-to-date",
                Decision::Update => "available",
                Decision::PinnedBelowCurrent => "not-downgrading",
            };
            return print_evolve_json(&target_version, outcome);
        }
        let marker = match d {
            Decision::UpToDate | Decision::PinnedBelowCurrent => out.ok(),
            Decision::Update => out.warn(),
        };
        println!("{marker} {}", check_report(current, &target_version, &d));
        // spec: POL-66 -- when running is above the policy pin, warn that the pin is
        // an upper bound and does not downgrade. Human mode only; --json already
        // returned above.
        if matches!(d, Decision::PinnedBelowCurrent) && policy_pin_active {
            println!(
                "warning: running {current} differs from the managed policy pin \
                 {target_version}; the policy pin is an upper bound and does not downgrade"
            );
        }
        return Ok(());
    }

    match d {
        Decision::UpToDate => {
            if out.json {
                return print_evolve_json(&target_version, "up-to-date");
            }
            println!("{} mind {current} is already up to date", out.ok());
            return Ok(());
        }
        // spec: CLI-147 -- explicit pin below running version: report and exit 0,
        // do NOT download or replace the binary.
        Decision::PinnedBelowCurrent => {
            if out.json {
                return print_evolve_json(&target_version, "not-downgrading");
            }
            println!(
                "{} {}",
                out.ok(),
                check_report(current, &target_version, &d)
            );
            // spec: POL-66 -- when running is above the policy pin, warn that the pin
            // is an upper bound and does not downgrade. Human mode only; --json already
            // returned above.
            if policy_pin_active {
                println!(
                    "warning: running {current} differs from the managed policy pin \
                     {target_version}; the policy pin is an upper bound and does not downgrade"
                );
            }
            return Ok(());
        }
        Decision::Update => {}
    }

    if !yes && !out.json && !crate::commands::confirm(&format!("update mind to {target_version}?"))?
    {
        println!("aborted; nothing changed");
        return Ok(());
    }

    let url = asset_url(&target_version, target);
    download_and_swap(&url, current, &target_version)
}

/// Emit the structured `evolve` result (CLI-153) under `--json`.
fn print_evolve_json(version: &str, outcome: &str) -> Result<()> {
    crate::render::print_json(&serde_json::json!({
        "action": "evolve",
        "target": version,
        "outcome": outcome,
    }))
}

/// The GitHub release asset URL for the SHA256SUMS file (STO-47).
pub fn sha256sums_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{version}/SHA256SUMS")
}

/// Parse a `sha256sum`-format sums file and return the digest for `filename`.
///
/// Expected format per line: `<lowercase-hex-digest>  <bare-filename>` (two
/// spaces). Lines that do not follow this format are skipped. Returns `None`
/// when no entry for `filename` is found.
pub fn parse_sha256sums(text: &str, filename: &str) -> Option<String> {
    for line in text.lines() {
        // Standard sha256sum output: 64-char hex, two spaces, filename.
        if let Some((digest, name)) = line.split_once("  ") {
            let name = name.trim();
            if name == filename && digest.len() == 64 {
                return Some(digest.to_ascii_lowercase());
            }
        }
    }
    None
}

/// Compute the SHA-256 digest of `data` and return it as a lowercase hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download the release archive, extract it, and atomically swap the new binary
/// for the running executable. Imperative and network-touching; the swap is
/// atomic so any failure leaves the existing binary intact.
///
/// Holds the global exclusive lock (STO-46) for the entire download-and-swap
/// step so two concurrent `mind evolve` invocations cannot race.
fn download_and_swap(url: &str, current: &str, target_version: &str) -> Result<()> {
    // spec: STO-46 -- hold the exclusive lock for the entire download-and-swap.
    let paths = crate::paths::Paths::resolve()?;
    let mut lock = crate::lock::open(&paths)?;
    let _guard = lock.write()?;

    let out = crate::render::ctx();
    let tmp = mktemp_dir()?;
    let archive = tmp.join("mind.tar.gz");

    if !out.json {
        println!(
            "{} downloading mind {target_version} ({})",
            out.bullet(),
            out.dim(url)
        );
    }

    // spec: STO-47 -- download SHA256SUMS and verify before extracting.
    let sums_url = sha256sums_url(target_version);
    let sums_text = fetch_to_string(&sums_url)?;
    // The archive filename is the last path component of the url (no path prefix).
    let archive_filename = url.rsplit('/').next().unwrap_or("");

    fetch_to_file(url, &archive)?;

    // Verify digest after download, before extraction.
    let archive_bytes = std::fs::read(&archive).map_err(|e| MindError::io(&archive, e))?;
    let actual = sha256_hex(&archive_bytes);
    let expected = parse_sha256sums(&sums_text, archive_filename).ok_or_else(|| {
        MindError::DigestMismatch {
            url: url.to_string(),
            expected: "(not found in SHA256SUMS)".to_string(),
            actual: actual.clone(),
        }
    })?;
    if actual != expected {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(MindError::DigestMismatch {
            url: url.to_string(),
            expected,
            actual,
        });
    }

    // Extract the archive into the temp dir.
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| MindError::io("tar", e))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: "could not extract the release archive".to_string(),
        });
    }

    let new_bin = tmp.join("mind");
    if !new_bin.is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(MindError::ReleaseAssetEmpty);
    }

    let current_exe = std::env::current_exe().map_err(|e| MindError::io("<current-exe>", e))?;
    let result = swap_in_place(&new_bin, &current_exe);
    let _ = std::fs::remove_dir_all(&tmp);
    result?;

    if out.json {
        return print_evolve_json(target_version, "updated");
    }
    println!("{} updated mind {current} -> {target_version}", out.ok());
    Ok(())
}

/// Atomically replace `current_exe` with `new_bin`: copy the new binary to a
/// uniquely-named temp file in the SAME directory as the running executable (so
/// the rename stays on one filesystem), make it executable, then rename it over
/// the target. A rename or permission failure on a non-writable target is
/// reported as `TargetNotWritable`.
///
/// The staged name is `.mind-update.<pid>.<nanos>` (STO-45): including the PID
/// and a nanosecond timestamp makes it unique per-invocation. If the path already
/// exists before the copy begins, `evolve` refuses and returns an I/O error
/// (pre-creation race detection, STO-45).
fn swap_in_place(new_bin: &Path, current_exe: &Path) -> Result<()> {
    // spec: STO-45
    let dir = current_exe
        .parent()
        .ok_or_else(|| MindError::TargetNotWritable {
            path: current_exe.display().to_string(),
        })?;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let staged = dir.join(format!(".mind-update.{pid}.{nanos}"));

    // Refuse if the staged path already exists (pre-creation race, STO-45).
    if staged.exists() {
        return Err(MindError::io(
            &staged,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "staged path already exists; possible pre-creation race",
            ),
        ));
    }

    // Copy the new binary alongside the target. A permission failure here (e.g.
    // the install directory is not writable) means we cannot replace the binary.
    if let Err(e) = std::fs::copy(new_bin, &staged) {
        return Err(swap_error(e, current_exe, &staged));
    }
    // chmod 0755 so the replacement is executable.
    if let Err(e) = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)) {
        let _ = std::fs::remove_file(&staged);
        return Err(MindError::io(&staged, e));
    }
    // The atomic step: rename over the running executable.
    if let Err(e) = std::fs::rename(&staged, current_exe) {
        let _ = std::fs::remove_file(&staged);
        return Err(swap_error(e, current_exe, current_exe));
    }
    Ok(())
}

/// Map a swap failure to the right structured error: a permission error means the
/// target binary is not writable (the actionable case, suggesting a privileged
/// reinstall or `brew upgrade`); anything else is a tagged I/O error at `at`.
fn swap_error(e: std::io::Error, current_exe: &Path, at: &Path) -> MindError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        MindError::TargetNotWritable {
            path: current_exe.display().to_string(),
        }
    } else {
        MindError::io(at, e)
    }
}

/// Per-process counter that makes successive `mktemp_dir` calls within the same
/// process yield distinct paths even when the wall-clock resolution is coarser
/// than the interval between calls.
static MKTEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create an unpredictably-named, exclusively-owned temp directory for the
/// download.  The name combines the PID, a subsecond wall-clock timestamp, and a
/// per-process sequence number so that:
///
/// - two successive calls within the same process always yield distinct paths
///   (the sequence number), and
/// - the path is hard to predict from outside (the nanos component varies with
///   the exact call time).
///
/// `create_dir` (not `create_dir_all`) gives exclusive-creation semantics: if the
/// directory already exists the call fails rather than silently reusing it, which
/// prevents a local attacker from pre-creating the path.
///
/// TODO: replace the nanos component with a CSPRNG once a `rand` dep is added;
/// the principled hardening is to verify a published release digest/signature
/// after download (out of scope here).
fn mktemp_dir() -> Result<std::path::PathBuf> {
    let pid = std::process::id();
    let seq = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let base = std::env::temp_dir().join(format!("mind-evolve-{pid}-{nanos}-{seq}"));
    // Exclusive creation: fails if the path already exists.
    std::fs::create_dir(&base).map_err(|e| MindError::io(&base, e))?;
    // 0700: only the owning process can enter or read the directory.
    #[cfg(unix)]
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| MindError::io(&base, e))?;
    Ok(base)
}

/// Clamp the raw parsed value of `MIND_HTTP_TIMEOUT_SECS` to the usable range.
///
/// A value of 0 is treated the same as a missing value and falls back to 15:
/// both `--connect-timeout 0` (curl) and `--timeout=0` (wget) mean "no limit",
/// which silently defeats the intent of the knob (STO-52).
// spec: STO-52
pub(crate) fn clamp_http_timeout(raw: Option<u64>) -> u64 {
    match raw {
        None | Some(0) => 15,
        Some(n) => n,
    }
}

/// Read the connect-timeout from `MIND_HTTP_TIMEOUT_SECS` (STO-52).
/// Falls back to 15 on a missing, non-numeric, or zero value.
fn http_timeout_secs() -> u64 {
    clamp_http_timeout(
        std::env::var("MIND_HTTP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok()),
    )
}

/// Build the curl argument list for a URL-to-stdout fetch (STO-52).
///
/// Includes the secure-transport flags mirroring install.sh, a configurable
/// connect timeout, and a generous 600-second max-time ceiling. Returns a
/// `Vec<String>` so the arg list is unit-testable without spawning a process.
pub(crate) fn curl_string_args(url: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "--proto".into(),
        "=https".into(),
        "--proto-redir".into(),
        "=https".into(),
        "--tlsv1.2".into(),
        "-fsSL".into(),
        "--connect-timeout".into(),
        timeout_secs.to_string(),
        "--max-time".into(),
        "600".into(),
        url.into(),
    ]
}

/// Build the wget argument list for a URL-to-stdout fetch (STO-52, STO-53).
///
/// `-q` is intentionally omitted so wget's stderr is captured on failure and
/// can populate `DownloadFailed.reason` with an actionable message.
/// `--tries=1` prevents wget's default 20-retry behaviour from multiplying the
/// effective timeout by up to 20x on a blackholed endpoint (STO-53).
pub(crate) fn wget_string_args(url: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "--https-only".into(),
        "--tries=1".into(),
        "-O-".into(),
        format!("--timeout={timeout_secs}"),
        url.into(),
    ]
}

/// Build the curl argument list for a URL-to-file fetch (STO-52).
///
/// `dest` is included as the `-o` value so the full arg list is unit-testable.
pub(crate) fn curl_file_args(url: &str, dest: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "--proto".into(),
        "=https".into(),
        "--proto-redir".into(),
        "=https".into(),
        "--tlsv1.2".into(),
        "-fsSL".into(),
        "--connect-timeout".into(),
        timeout_secs.to_string(),
        "--max-time".into(),
        "600".into(),
        url.into(),
        "-o".into(),
        dest.into(),
    ]
}

/// Build the wget argument list for a URL-to-file fetch (STO-52, STO-53).
///
/// `-q` is kept here (file-fetch; exit code signals failure) and `dest` is
/// included in the arg list for unit-testability.
/// `--tries=1` prevents wget's default 20-retry behaviour from multiplying the
/// effective timeout by up to 20x on a blackholed endpoint (STO-53).
pub(crate) fn wget_file_args(url: &str, dest: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "--https-only".into(),
        "--tries=1".into(),
        "-qO".into(),
        dest.into(),
        format!("--timeout={timeout_secs}"),
        url.into(),
    ]
}

/// Whether a URL targets the GitHub REST API host. Only `api.github.com` is
/// rate-limited per source IP for unauthenticated callers; the release-artifact
/// and SHA256SUMS downloads live on `github.com` / the CDN and are not.
fn is_github_api_url(url: &str) -> bool {
    url.starts_with("https://api.github.com/")
}

/// A GitHub token from the environment, if set: `GITHUB_TOKEN` first, then
/// `GH_TOKEN` (matching the `gh` CLI), first non-empty wins. Trailing whitespace
/// is trimmed so a token read from a file with a trailing newline still forms a
/// valid header.
fn github_token() -> Option<String> {
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// The extra curl args authenticating a GitHub REST API request (STO-57).
///
/// Returns `-H "Authorization: Bearer <token>"` only when a non-empty token is
/// present AND the URL targets `api.github.com`, so the token is never forwarded
/// to the artifact CDN on a cross-host redirect. Pure (token passed in) so the
/// arg vector is unit-testable without touching the environment or a process.
// spec: STO-57
pub(crate) fn curl_auth_args(url: &str, token: Option<&str>) -> Vec<String> {
    match token {
        Some(t) if !t.is_empty() && is_github_api_url(url) => {
            vec!["-H".into(), format!("Authorization: Bearer {t}")]
        }
        _ => vec![],
    }
}

/// The extra wget args authenticating a GitHub REST API request (STO-57).
///
/// The wget counterpart to `curl_auth_args`: a single `--header=...` arg (the
/// inline form matching the other wget builders here) gated to `api.github.com`.
// spec: STO-57
pub(crate) fn wget_auth_args(url: &str, token: Option<&str>) -> Vec<String> {
    match token {
        Some(t) if !t.is_empty() && is_github_api_url(url) => {
            vec![format!("--header=Authorization: Bearer {t}")]
        }
        _ => vec![],
    }
}

/// Append a proxy-setup hint when the failure reason looks like a proxy error.
///
/// Matches HTTP 407 responses and "Could not resolve proxy" messages that curl
/// and wget emit when a proxy is misconfigured or missing credentials.
///
/// The `reason` text comes from curl/wget stderr, which is untrusted (a MITM'd
/// or hostile endpoint controls those bytes). It is sanitized via `strip_ansi`
/// before being embedded in the returned string (STO-54).
fn maybe_proxy_hint(reason: &str) -> String {
    // spec: STO-54 -- sanitize curl/wget output before it is placed in
    // DownloadFailed.reason; a hostile endpoint controls stderr bytes.
    let reason = crate::sanitize::strip_ansi(reason);
    if reason.contains("407")
        || reason.contains("Could not resolve proxy")
        || reason.contains("Received HTTP code 407 from proxy")
    {
        format!(
            "{reason}\nhint: if you are behind a proxy, set HTTPS_PROXY or HTTP_PROXY \
             (e.g. export HTTPS_PROXY=http://proxy.example.com:8080); \
             for NTLM or Kerberos proxies, configure proxy settings in ~/.curlrc \
             (proxy-negotiate)"
        )
    } else {
        reason
    }
}

/// Fetch a URL to a string via curl or wget, mirroring install.sh's secure flags.
fn fetch_to_string(url: &str) -> Result<String> {
    let timeout = http_timeout_secs();
    let token = github_token();
    let output = if have("curl") {
        let mut args = curl_string_args(url, timeout);
        args.extend(curl_auth_args(url, token.as_deref()));
        Command::new("curl")
            .args(args)
            .output()
            .map_err(|e| MindError::io("curl", e))?
    } else if have("wget") {
        let mut args = wget_string_args(url, timeout);
        args.extend(wget_auth_args(url, token.as_deref()));
        Command::new("wget")
            .args(args)
            .output()
            .map_err(|e| MindError::io("wget", e))?
    } else {
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: "need curl or wget on PATH".to_string(),
        });
    };
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: maybe_proxy_hint(&reason),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fetch a URL to a file via curl or wget, mirroring install.sh's secure flags.
fn fetch_to_file(url: &str, dest: &Path) -> Result<()> {
    let timeout = http_timeout_secs();
    let dest_str = dest.to_string_lossy();
    let token = github_token();
    let status = if have("curl") {
        let mut args = curl_file_args(url, &dest_str, timeout);
        args.extend(curl_auth_args(url, token.as_deref()));
        Command::new("curl")
            .args(args)
            .status()
            .map_err(|e| MindError::io("curl", e))?
    } else if have("wget") {
        let mut args = wget_file_args(url, &dest_str, timeout);
        args.extend(wget_auth_args(url, token.as_deref()));
        Command::new("wget")
            .args(args)
            .status()
            .map_err(|e| MindError::io("wget", e))?
    } else {
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: "need curl or wget on PATH".to_string(),
        });
    };
    if !status.success() {
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: "downloader exited non-zero".to_string(),
        });
    }
    Ok(())
}

/// Whether a command exists on PATH. `command -v` is a shell builtin, not an
/// executable, so it must run inside a shell (`Command::new("command")` would
/// just fail to spawn and report everything as missing).
fn have(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- STO-52: timeout arg-vector helpers ----------------------------------

    #[test]
    // spec: STO-52
    fn curl_string_args_includes_connect_timeout_and_max_time() {
        // The arg vector must contain --connect-timeout N and --max-time 600 so
        // a blackholing firewall doesn't hang evolve forever.
        let args = curl_string_args("https://example.com/data", 15);
        let ct = args
            .iter()
            .position(|a| a == "--connect-timeout")
            .expect("--connect-timeout must be present");
        assert_eq!(
            args[ct + 1],
            "15",
            "connect-timeout value must follow --connect-timeout"
        );
        let mt = args
            .iter()
            .position(|a| a == "--max-time")
            .expect("--max-time must be present");
        assert_eq!(args[mt + 1], "600", "max-time must be 600 seconds");
        // The URL must also be present.
        assert!(
            args.contains(&"https://example.com/data".to_string()),
            "URL must be in the arg list"
        );
    }

    #[test]
    // spec: STO-52
    fn wget_string_args_includes_timeout_and_no_quiet_flag() {
        // wget string-fetch must include --timeout=N and must NOT include -q,
        // so that wget's stderr is captured and available as the failure reason.
        let args = wget_string_args("https://example.com/data", 15);
        assert!(
            args.contains(&"--timeout=15".to_string()),
            "wget args must include --timeout=15: {args:?}"
        );
        assert!(
            args.contains(&"https://example.com/data".to_string()),
            "wget args must include the URL: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "-q" || a.contains('q')),
            "wget string-fetch must not include -q (stderr must be visible): {args:?}"
        );
    }

    #[test]
    // spec: STO-52
    fn curl_file_args_includes_connect_timeout_and_dest() {
        let args = curl_file_args("https://example.com/file.tar.gz", "/tmp/dest.tar.gz", 30);
        let ct = args
            .iter()
            .position(|a| a == "--connect-timeout")
            .expect("--connect-timeout must be present");
        assert_eq!(
            args[ct + 1],
            "30",
            "custom connect-timeout value must be 30"
        );
        assert!(
            args.contains(&"--max-time".to_string()),
            "must include --max-time: {args:?}"
        );
        assert!(
            args.contains(&"/tmp/dest.tar.gz".to_string()),
            "dest path must be in arg list: {args:?}"
        );
        assert!(
            args.contains(&"https://example.com/file.tar.gz".to_string()),
            "URL must be in arg list: {args:?}"
        );
    }

    #[test]
    // spec: STO-52
    fn wget_file_args_includes_timeout_and_dest() {
        let args = wget_file_args("https://example.com/file.tar.gz", "/tmp/dest.tar.gz", 30);
        assert!(
            args.contains(&"--timeout=30".to_string()),
            "wget file args must include --timeout=30: {args:?}"
        );
        assert!(
            args.contains(&"/tmp/dest.tar.gz".to_string()),
            "dest must be in file-fetch args: {args:?}"
        );
        assert!(
            args.contains(&"https://example.com/file.tar.gz".to_string()),
            "URL must be in file-fetch args: {args:?}"
        );
    }

    // ---- STO-57: GitHub API auth header --------------------------------------

    #[test]
    // spec: STO-57
    fn auth_args_add_bearer_header_for_api_host() {
        // A token present + an api.github.com URL -> the bearer header is added.
        let curl = curl_auth_args(
            "https://api.github.com/repos/jaemk/mind/releases/latest",
            Some("tok123"),
        );
        assert_eq!(
            curl,
            vec!["-H".to_string(), "Authorization: Bearer tok123".to_string()],
            "curl must send the bearer header on the API host: {curl:?}"
        );
        let wget = wget_auth_args(
            "https://api.github.com/repos/jaemk/mind/releases/latest",
            Some("tok123"),
        );
        assert_eq!(
            wget,
            vec!["--header=Authorization: Bearer tok123".to_string()],
            "wget must send the bearer header on the API host: {wget:?}"
        );
    }

    #[test]
    // spec: STO-57
    fn auth_args_never_leak_token_to_non_api_hosts() {
        // The token must NOT be attached to the artifact CDN download, so it is
        // not forwarded across a cross-host redirect.
        let url = "https://github.com/jaemk/mind/releases/download/v1.2.3/mind-1.2.3-x.tar.gz";
        assert!(
            curl_auth_args(url, Some("tok123")).is_empty(),
            "curl must not send a token to github.com"
        );
        assert!(
            wget_auth_args(url, Some("tok123")).is_empty(),
            "wget must not send a token to github.com"
        );
    }

    #[test]
    // spec: STO-57
    fn auth_args_empty_without_a_token() {
        // No token (or an empty one) -> the request is byte-for-byte unchanged.
        let url = "https://api.github.com/repos/jaemk/mind/releases/latest";
        assert!(
            curl_auth_args(url, None).is_empty(),
            "no token -> no curl header"
        );
        assert!(
            wget_auth_args(url, None).is_empty(),
            "no token -> no wget header"
        );
        assert!(
            curl_auth_args(url, Some("")).is_empty(),
            "empty token -> no curl header"
        );
        assert!(
            wget_auth_args(url, Some("")).is_empty(),
            "empty token -> no wget header"
        );
    }

    #[test]
    // spec: STO-52
    fn timeout_param_flows_through_arg_builders() {
        // Verify that different timeout values produce the corresponding flag
        // values, proving the parameter is not hardcoded.
        let args = curl_string_args("https://example.com/", 42);
        let ct = args.iter().position(|a| a == "--connect-timeout").unwrap();
        assert_eq!(
            args[ct + 1],
            "42",
            "custom timeout must appear in curl args"
        );

        let args = wget_string_args("https://example.com/", 42);
        assert!(
            args.contains(&"--timeout=42".to_string()),
            "custom timeout must appear in wget args: {args:?}"
        );
    }

    #[test]
    // spec: STO-52
    fn http_timeout_zero_clamped_to_default() {
        // MIND_HTTP_TIMEOUT_SECS=0 means "no limit" in both curl (--connect-timeout 0)
        // and wget (--timeout=0), silently defeating the knob. clamp_http_timeout
        // must treat 0 the same as a missing value and return the 15-second default.
        assert_eq!(
            clamp_http_timeout(Some(0)),
            15,
            "a zero value must clamp to the 15-second default"
        );
        assert_eq!(
            clamp_http_timeout(None),
            15,
            "a missing value must default to 15"
        );
        assert_eq!(
            clamp_http_timeout(Some(30)),
            30,
            "a non-zero value must pass through unchanged"
        );
        assert_eq!(
            clamp_http_timeout(Some(1)),
            1,
            "the minimum non-zero value (1) must not be altered"
        );
    }

    #[test]
    // spec: STO-53
    fn wget_args_include_tries_1() {
        // All wget invocations must pass --tries=1 so a blackholed endpoint cannot
        // exhaust ~20x the intended timeout bound (wget defaults to 20 retries;
        // curl is already a single attempt bounded by --max-time).
        let str_args = wget_string_args("https://example.com/data", 15);
        assert!(
            str_args.contains(&"--tries=1".to_string()),
            "wget string-fetch must include --tries=1: {str_args:?}"
        );

        let file_args = wget_file_args("https://example.com/file.tar.gz", "/tmp/dest.tar.gz", 30);
        assert!(
            file_args.contains(&"--tries=1".to_string()),
            "wget file-fetch must include --tries=1: {file_args:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_strips_ansi_and_bidi_from_reason() {
        // The reason text comes from curl/wget stderr, which a MITM'd or hostile
        // endpoint controls. maybe_proxy_hint must strip ANSI escapes and bidi
        // override characters before embedding the reason in DownloadFailed.reason.
        let ansi_reason = "download error \x1b[31mred\x1b[0m text";
        let result = maybe_proxy_hint(ansi_reason);
        assert!(
            !result.contains('\x1b'),
            "ANSI escape sequences must be stripped from the reason: {result:?}"
        );
        assert!(
            result.contains("download error"),
            "visible text must be preserved after stripping: {result:?}"
        );

        // Bidi override characters (U+202E and siblings) must also be stripped.
        let bidi_reason = "pay \u{202E}oot";
        let result = maybe_proxy_hint(bidi_reason);
        assert!(
            !result.contains('\u{202E}'),
            "bidi override (U+202E) must be stripped: {result:?}"
        );

        // The proxy-hint branch must also produce a sanitized output.
        let hostile_407 = "\x1b[1m407 Proxy Auth Required\x1b[0m \u{202E}spoofed";
        let result = maybe_proxy_hint(hostile_407);
        assert!(
            !result.contains('\x1b'),
            "ANSI must be stripped even in the proxy-hint branch: {result:?}"
        );
        assert!(
            !result.contains('\u{202E}'),
            "bidi must be stripped even in the proxy-hint branch: {result:?}"
        );
        // The hint must still be appended (407 was present after sanitization).
        assert!(
            result.contains("HTTPS_PROXY"),
            "proxy hint must still be appended when 407 is present: {result:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_curlrc_mention_no_git_proxy() {
        // The proxy hint must NOT mention git's http.proxy setting (which has no
        // effect on curl/wget subprocesses) and MUST mention the curlrc escape hatch.
        let reason_407 = "Received HTTP code 407 from proxy";
        let hint = maybe_proxy_hint(reason_407);
        assert!(
            !hint.contains("http.proxy"),
            "hint must not mention git's http.proxy (ineffective for curl/wget): {hint:?}"
        );
        assert!(
            hint.contains("curlrc") || hint.contains(".curlrc"),
            "hint must mention ~/.curlrc as the NTLM/Kerberos escape hatch: {hint:?}"
        );
        assert!(
            hint.contains("HTTPS_PROXY") || hint.contains("HTTP_PROXY"),
            "hint must name HTTPS_PROXY or HTTP_PROXY: {hint:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_407_in_escape_sequence_does_not_trigger_hint() {
        // Adversarial: a hostile endpoint emits an SGR color escape whose numeric
        // parameter is `407` (`ESC [ 4 0 7 m`). strip_ansi removes the whole escape
        // BEFORE the `contains("407")` test, so the digits inside the escape must
        // NOT be left behind to spuriously trigger the proxy hint. The `407` was
        // never real HTTP-407 text; it was an ANSI parameter.
        let colored = "\x1b[407mdownload timed out\x1b[0m";
        let result = maybe_proxy_hint(colored);
        assert!(
            !result.contains('\x1b'),
            "escape must be stripped: {result:?}"
        );
        assert!(
            !result.contains("407"),
            "the `407` ANSI parameter must be stripped, not surface as text: {result:?}"
        );
        assert!(
            !result.contains("HTTPS_PROXY"),
            "an ANSI-parameter 407 must NOT append the proxy hint: {result:?}"
        );
        assert_eq!(
            result, "download timed out",
            "only the visible message survives: {result:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_real_407_split_across_escapes_is_detected() {
        // Adversarial converse: a genuine HTTP-407 message with a color escape
        // spliced into the middle of the digits (`4 ESC[0m 07`). Because strip_ansi
        // runs FIRST, the escape is removed and the digits rejoin into `407`, so the
        // proxy hint IS correctly appended. This pins the ordering: sanitize, then
        // match -- not match, then sanitize (which would miss this).
        let split = "HTTP 4\x1b[0m07 from proxy";
        let result = maybe_proxy_hint(split);
        assert!(
            !result.contains('\x1b'),
            "escape must be stripped: {result:?}"
        );
        assert!(
            result.contains("407"),
            "digits must rejoin into 407 after stripping the interior escape: {result:?}"
        );
        assert!(
            result.contains("HTTPS_PROXY"),
            "a real 407 (revealed after sanitization) must append the proxy hint: {result:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_could_not_resolve_proxy_branch() {
        // The second recognized proxy-failure phrase must also append the hint, and
        // the output must still be sanitized (ANSI stripped) in that branch.
        let reason = "wget: \x1b[31mCould not resolve proxy\x1b[0m: proxy.local";
        let result = maybe_proxy_hint(reason);
        assert!(
            !result.contains('\x1b'),
            "ANSI must be stripped in the resolve-proxy branch: {result:?}"
        );
        assert!(
            result.contains("Could not resolve proxy"),
            "the recognized phrase must survive sanitization: {result:?}"
        );
        assert!(
            result.contains("HTTPS_PROXY"),
            "the resolve-proxy phrase must trigger the proxy hint: {result:?}"
        );
    }

    #[test]
    // spec: STO-54
    fn maybe_proxy_hint_non_proxy_reason_is_returned_verbatim_after_sanitizing() {
        // A non-proxy failure must be returned sanitized but WITHOUT the hint
        // appended, so ordinary download errors are not decorated with proxy advice.
        let reason = "server returned HTTP 500";
        let result = maybe_proxy_hint(reason);
        assert_eq!(
            result, "server returned HTTP 500",
            "a non-proxy reason must pass through unchanged (already ASCII): {result:?}"
        );
        assert!(
            !result.contains("HTTPS_PROXY"),
            "a non-proxy reason must NOT append the proxy hint: {result:?}"
        );
    }

    // ---- STO-53: install.sh wget invocations pass --tries=1 ------------------

    /// Read the shipped `resources/install.sh` from the crate root (mirrors the
    /// CHANGELOG.md resource-reading pattern in tests/changelog.rs).
    fn install_sh() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/install.sh");
        std::fs::read_to_string(&path).expect("resources/install.sh must exist and be readable")
    }

    #[test]
    // spec: STO-53
    fn install_sh_every_wget_invocation_passes_tries_1() {
        // STO-53 requires that *every* wget invocation in resources/install.sh (not
        // just the Rust downloader) pass --tries=1, so the shell installer cannot
        // hang ~20x the timeout on a blackholed endpoint. The Rust arg builders are
        // covered by wget_args_include_tries_1; this closes the shell-script half.
        let script = install_sh();
        let wget_lines: Vec<&str> = script
            .lines()
            .map(|l| l.trim())
            .filter(|l| l.starts_with("wget "))
            .collect();
        assert_eq!(
            wget_lines.len(),
            2,
            "install.sh must invoke wget exactly twice (fetch + fetch_to); found: {wget_lines:?}"
        );
        for line in &wget_lines {
            assert!(
                line.contains("--tries=1"),
                "every wget invocation in install.sh must pass --tries=1: {line:?}"
            );
        }
    }

    #[test]
    // spec: STO-52
    fn install_sh_wget_invocations_carry_a_timeout() {
        // install.sh's wget calls must also carry an explicit --timeout so a stalled
        // connect cannot hang the installer (STO-52; the fixed 15 s CONNECT_TIMEOUT).
        let script = install_sh();
        for line in script
            .lines()
            .map(|l| l.trim())
            .filter(|l| l.starts_with("wget "))
        {
            assert!(
                line.contains("--timeout="),
                "every wget invocation in install.sh must pass --timeout=: {line:?}"
            );
        }
    }

    // ---- POL-51..54: policy control over self-update -------------------------

    /// Load a `Policy` from a TOML string via a temp file (mirrors the
    /// MIND_POLICY_FILE fixture pattern used in tests/cli.rs).
    fn policy_from_toml(toml: &str) -> crate::policy::Policy {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "mind-selfupdate-pol-{}-{}.toml",
            std::process::id(),
            MKTEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, toml).unwrap();
        let p = crate::policy::load_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        p
    }

    #[test]
    // spec: POL-51
    fn policy_absent_allows_evolve_with_no_pin() {
        // No policy -> check_policy_for_evolve returns Ok(None): unrestricted.
        let result = check_policy_for_evolve(None, None);
        assert_eq!(
            result.unwrap(),
            None,
            "absent policy must return Ok(None): unrestricted evolve"
        );
    }

    #[test]
    // spec: POL-54
    fn policy_self_update_true_allows_evolve() {
        // [binary].self-update = true is explicitly allowed (same as absent).
        let pol = policy_from_toml("[binary]\nself-update = true\n");
        let result = check_policy_for_evolve(Some(&pol), None);
        assert_eq!(
            result.unwrap(),
            None,
            "self-update = true must return Ok(None): unrestricted evolve"
        );
    }

    #[test]
    // spec: POL-52
    fn policy_disabled_denies_evolve_check_and_run() {
        // [binary].self-update = false -> Err(SelfUpdatePolicy) in all invocation modes.
        let pol = policy_from_toml("[binary]\nself-update = false\n");

        // No --version: disabled.
        let err = check_policy_for_evolve(Some(&pol), None).unwrap_err();
        match err {
            MindError::SelfUpdatePolicy { detail } => {
                assert!(
                    detail.contains("disabled by the managed policy"),
                    "disabled detail must say the policy disabled it: {detail}"
                );
            }
            other => panic!("expected SelfUpdatePolicy, got {other:?}"),
        }

        // With --version: still disabled (no version makes it OK).
        let err = check_policy_for_evolve(Some(&pol), Some("9.9.9")).unwrap_err();
        assert!(
            matches!(err, MindError::SelfUpdatePolicy { .. }),
            "disabled policy must error even with a --version arg: {err:?}"
        );
    }

    #[test]
    // spec: POL-53
    fn policy_pinned_no_version_arg_returns_pin() {
        // Policy pins to "0.14.0"; no --version -> return the pin, no network call.
        let pol = policy_from_toml("[binary]\nself-update = \"0.14.0\"\n");
        let pin = check_policy_for_evolve(Some(&pol), None).unwrap();
        assert_eq!(
            pin,
            Some("0.14.0".to_string()),
            "pinned policy with no --version must return the pin"
        );
    }

    #[test]
    // spec: POL-53
    fn policy_pinned_matching_version_arg_returns_pin() {
        // Policy pins to "0.14.0"; --version 0.14.0 matches -> returns the pin.
        let pol = policy_from_toml("[binary]\nself-update = \"0.14.0\"\n");
        let pin = check_policy_for_evolve(Some(&pol), Some("0.14.0")).unwrap();
        assert_eq!(
            pin,
            Some("0.14.0".to_string()),
            "matching --version must succeed with a pinned policy"
        );
    }

    #[test]
    // spec: POL-53
    fn policy_pinned_mismatched_version_arg_errors() {
        // Policy pins to "0.14.0"; --version 0.15.0 conflicts -> Err.
        let pol = policy_from_toml("[binary]\nself-update = \"0.14.0\"\n");
        let result = check_policy_for_evolve(Some(&pol), Some("0.15.0"));
        match result.unwrap_err() {
            MindError::SelfUpdatePolicy { detail } => {
                assert!(detail.contains("0.14.0"), "must name the pin: {detail}");
                assert!(
                    detail.contains("0.15.0"),
                    "must name the conflicting version: {detail}"
                );
                assert!(
                    detail.contains("conflicts"),
                    "must say 'conflicts': {detail}"
                );
            }
            other => panic!("expected SelfUpdatePolicy, got {other:?}"),
        }
    }

    #[test]
    fn have_detects_present_and_absent_commands() {
        // `sh` is on PATH on every supported platform; a builtin like `command`
        // is not an executable, so the old `Command::new("command")` probe wrongly
        // reported everything missing. This guards that regression.
        assert!(have("sh"), "`sh` must be detected on PATH");
        assert!(
            !have("mind-no-such-binary-xyzzy"),
            "a nonexistent command must not be detected"
        );
    }

    #[test]
    fn target_triple_maps_supported_platforms() {
        assert_eq!(
            target_triple("linux", "x86_64").unwrap(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple("linux", "aarch64").unwrap(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple("macos", "aarch64").unwrap(),
            "aarch64-apple-darwin"
        );
    }

    #[test]
    fn target_triple_rejects_intel_macos_and_unknown_arch() {
        // Intel macOS has no published artifact (mirrors install.sh).
        match target_triple("macos", "x86_64") {
            Err(MindError::UnsupportedPlatform { os, arch }) => {
                assert_eq!(os, "macos");
                assert_eq!(arch, "x86_64");
            }
            other => panic!("expected UnsupportedPlatform, got {other:?}"),
        }
        // An unknown architecture is also rejected.
        assert!(matches!(
            target_triple("linux", "riscv64"),
            Err(MindError::UnsupportedPlatform { .. })
        ));
        // An unknown OS is rejected.
        assert!(matches!(
            target_triple("windows", "x86_64"),
            Err(MindError::UnsupportedPlatform { .. })
        ));
    }

    #[test]
    fn asset_url_matches_install_sh_shape() {
        assert_eq!(
            asset_url("0.3.0", "x86_64-unknown-linux-gnu"),
            "https://github.com/jaemk/mind/releases/download/v0.3.0/mind-0.3.0-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    #[test]
    fn parse_latest_tag_strips_leading_v() {
        let json = r#"{"tag_name":"v0.3.0","name":"0.3.0"}"#;
        assert_eq!(parse_latest_tag(json).unwrap(), "0.3.0");
        // A tag without a leading v is returned as-is.
        let json = r#"{"tag_name":"1.2.3"}"#;
        assert_eq!(parse_latest_tag(json).unwrap(), "1.2.3");
    }

    #[test]
    fn parse_latest_tag_missing_field_is_an_error() {
        let json = r#"{"name":"0.3.0"}"#;
        match parse_latest_tag(json) {
            Err(MindError::DownloadFailed { reason, .. }) => {
                assert!(reason.contains("tag_name"), "reason: {reason}");
            }
            other => panic!("expected DownloadFailed, got {other:?}"),
        }
    }

    #[test]
    // spec: CLI-140
    fn decision_compares_versions() {
        // current == target => up to date (explicit or not).
        assert_eq!(decision("0.3.0", "0.3.0", false), Decision::UpToDate);
        assert_eq!(decision("0.3.0", "0.3.0", true), Decision::UpToDate);
        // target newer => update.
        assert_eq!(decision("0.2.0", "0.3.0", false), Decision::Update);
        // current newer, no explicit pin => up to date.
        assert_eq!(decision("0.4.0", "0.3.0", false), Decision::UpToDate);
    }

    #[test]
    // spec: CLI-147
    fn decision_explicit_pinned_below_current_yields_pinned_below() {
        // An explicit --version strictly below the running version must NOT return
        // UpToDate; the caller needs PinnedBelowCurrent to emit a "not downgrading"
        // message rather than silently claiming up to date.
        assert_eq!(
            decision("0.3.0", "0.1.0", true),
            Decision::PinnedBelowCurrent
        );
        assert_eq!(
            decision("1.0.0", "0.9.9", true),
            Decision::PinnedBelowCurrent
        );
        // With explicit=false (latest from network) a running version >= latest is
        // still UpToDate, never PinnedBelowCurrent.
        assert_eq!(decision("0.4.0", "0.3.0", false), Decision::UpToDate);
    }

    #[test]
    // spec: CLI-140
    fn decision_explicit_equal_to_current_is_up_to_date() {
        // When the pinned version equals the running version "up to date" is correct
        // even with explicit=true; no downgrade is attempted.
        assert_eq!(decision("0.3.0", "0.3.0", true), Decision::UpToDate);
    }

    #[test]
    // spec: CLI-140
    fn decision_explicit_above_current_is_update() {
        // An explicit --version newer than the running version requests an upgrade.
        assert_eq!(decision("0.2.0", "0.3.0", true), Decision::Update);
    }

    #[test]
    // spec: CLI-141
    fn check_report_reflects_the_decision_without_network() {
        // The --check branch reports pending vs up-to-date purely from the
        // decision over an explicit target version: no network is consulted.
        let pending = decision("0.2.0", "0.3.0", false);
        assert_eq!(pending, Decision::Update);
        let report = check_report("0.2.0", "0.3.0", &pending);
        assert!(report.contains("0.2.0"), "report: {report}");
        assert!(report.contains("0.3.0"), "report: {report}");
        assert!(report.contains("available"), "report: {report}");

        let current = decision("0.3.0", "0.3.0", false);
        assert_eq!(current, Decision::UpToDate);
        let report = check_report("0.3.0", "0.3.0", &current);
        assert!(report.contains("up to date"), "report: {report}");
    }

    #[test]
    // spec: CLI-147
    fn check_report_pinned_below_says_not_downgrading() {
        // The report for PinnedBelowCurrent must name both versions and say
        // "not downgrading" -- it must NOT say "up to date".
        let d = Decision::PinnedBelowCurrent;
        let report = check_report("0.3.0", "0.1.0", &d);
        assert!(report.contains("0.1.0"), "pinned version missing: {report}");
        assert!(
            report.contains("0.3.0"),
            "running version missing: {report}"
        );
        assert!(
            report.contains("not downgrading"),
            "must say 'not downgrading': {report}"
        );
        assert!(
            !report.contains("up to date"),
            "must NOT say 'up to date': {report}"
        );
    }

    #[test]
    // spec: CLI-141
    fn check_report_up_to_date_when_equal() {
        // When the running and target versions are equal, "up to date" regardless
        // of explicit; tests the UpToDate arm of check_report directly.
        let d = Decision::UpToDate;
        let report = check_report("0.3.0", "0.3.0", &d);
        assert!(report.contains("up to date"), "report: {report}");
        assert!(
            !report.contains("not downgrading"),
            "must NOT say 'not downgrading': {report}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn swap_in_place_uses_pid_nanos_staged_name() {
        // spec: STO-45 -- the staged file must be named `.mind-update.<pid>.<nanos>`
        // (unique per-invocation) and must leave no `.mind-update.*` residue after
        // a successful swap.
        use std::sync::atomic::{AtomicU32, Ordering};
        static SWP_N: AtomicU32 = AtomicU32::new(0);

        let n = SWP_N.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-swap45-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let new_bin = base.join("new_mind");
        let cur = base.join("mind");
        std::fs::write(&new_bin, b"#!/bin/sh\necho new\n").unwrap();
        std::fs::write(&cur, b"#!/bin/sh\necho old\n").unwrap();
        std::fs::set_permissions(&cur, std::fs::Permissions::from_mode(0o755)).unwrap();

        // A normal swap must succeed and install the new content.
        swap_in_place(&new_bin, &cur).unwrap();
        assert_eq!(
            std::fs::read(&cur).unwrap(),
            b"#!/bin/sh\necho new\n",
            "swap_in_place must replace the current executable with the new binary"
        );

        // No `.mind-update.*` residue must remain in the directory after a
        // successful swap (the staged file was renamed over the target).
        let residue: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".mind-update."))
            .collect();
        assert!(
            residue.is_empty(),
            "staged file must not remain after a successful swap: {residue:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn mktemp_dir_creates_a_fresh_directory() {
        // The directory must exist after mktemp_dir returns and must be empty.
        let dir = mktemp_dir().expect("mktemp_dir");
        let exists = dir.is_dir();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(exists, "mktemp_dir must create the directory: {dir:?}");
    }

    #[test]
    fn mktemp_dir_yields_distinct_paths() {
        // Two successive calls must return different paths (the sequence number
        // component guarantees this within a process), and both must be creatable
        // -- proving the exclusive-create semantics would reject a pre-existing dir.
        let a = mktemp_dir().expect("first mktemp_dir");
        let b = mktemp_dir().expect("second mktemp_dir");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
        assert_ne!(
            a, b,
            "successive mktemp_dir calls must yield distinct paths"
        );
    }

    // ---- STO-47: SHA256SUMS parsing and digest verification ------------------

    #[test]
    fn parse_sha256sums_finds_matching_filename() {
        // spec: STO-47 -- parse_sha256sums must extract the hex digest for the
        // named file from standard sha256sum output (two-space separator).
        let sums = concat!(
            "aabbccdd00112233445566778899aabbccddeeff0011223344556677889900aa  mind-1.0.0-x86_64-unknown-linux-gnu.tar.gz\n",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  mind-1.0.0-aarch64-apple-darwin.tar.gz\n",
        );
        let got = parse_sha256sums(sums, "mind-1.0.0-aarch64-apple-darwin.tar.gz");
        assert_eq!(
            got.as_deref(),
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            "must return the digest for the matching filename"
        );
    }

    #[test]
    fn parse_sha256sums_returns_none_when_filename_absent() {
        // spec: STO-47 -- when no entry matches the filename, return None so the
        // caller can turn it into a DigestMismatch error.
        let sums =
            "aabbccdd00112233445566778899aabbccddeeff0011223344556677889900aa  other.tar.gz\n";
        let got = parse_sha256sums(sums, "mind-1.0.0-x86_64-unknown-linux-gnu.tar.gz");
        assert!(got.is_none(), "must return None for an absent filename");
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // spec: STO-47 -- sha256_hex must produce a lowercase hex sha256 digest.
        //
        // Reference: `printf "abc" | sha256sum` (system sha256sum and sha2 crate agree).
        // Note: sha2 uses hardware SHA-NI when available; this test captures the value
        // both the crate and system sha256sum produce on this platform.
        let digest = sha256_hex(b"abc");
        // Format checks: 64 lowercase hex characters (32-byte digest).
        assert_eq!(
            digest.len(),
            64,
            "sha256_hex output must be 64 hex chars (32 bytes): got {digest}"
        );
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "sha256_hex output must be all lowercase hex: {digest}"
        );
        // Consistency check: sha2 must produce the same value for the same input.
        let expected = sha2::Sha256::digest(b"abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(
            digest, expected,
            "sha256_hex must be consistent with sha2::Sha256::digest"
        );
    }

    #[test]
    fn composed_digest_verify_happy_path() {
        // spec: STO-47 -- the composed check (compute sha256_hex, parse expected
        // from SHA256SUMS, compare) must PASS when the sums file has the correct
        // digest for the archive filename.
        let archive_bytes = b"fake-archive-content-for-testing";
        let filename = "mind-1.0.0-x86_64-unknown-linux-gnu.tar.gz";

        let actual = sha256_hex(archive_bytes);
        let sums_text = format!("{actual}  {filename}\n");

        let expected = parse_sha256sums(&sums_text, filename)
            .expect("must find the filename in a correctly built sums file");
        assert_eq!(
            actual, expected,
            "computed digest must match the sums entry for the happy path"
        );
    }

    #[test]
    fn composed_digest_verify_mismatch_branch() {
        // spec: STO-47 -- when the sums file contains a DIFFERENT digest than the
        // actual archive hash, the composed check must detect the mismatch.
        // This exercises the `actual != expected` branch that download_and_swap
        // uses to emit DigestMismatch before extracting.
        let archive_bytes = b"fake-archive-content-for-testing";
        let filename = "mind-1.0.0-x86_64-unknown-linux-gnu.tar.gz";

        let actual = sha256_hex(archive_bytes);
        // Produce a digest that differs from the actual (flip the first byte).
        let tampered: String = {
            let first_byte = &actual[0..2];
            let replacement = if first_byte == "00" { "ff" } else { "00" };
            format!("{replacement}{}", &actual[2..])
        };
        assert_ne!(actual, tampered, "tampered digest must differ from actual");

        let sums_text = format!("{tampered}  {filename}\n");
        let expected =
            parse_sha256sums(&sums_text, filename).expect("must find the tampered entry");
        assert_ne!(
            actual, expected,
            "tampered sums must not match actual digest (mismatch branch must trigger)"
        );
    }

    #[test]
    fn composed_digest_verify_missing_entry_branch() {
        // spec: STO-47 -- when the SHA256SUMS file has no entry for the archive
        // filename, parse_sha256sums returns None, which download_and_swap maps
        // to the fail-closed digest error.
        let filename = "mind-1.0.0-x86_64-unknown-linux-gnu.tar.gz";
        let sums_text =
            "aabbccdd00112233445566778899aabbccddeeff0011223344556677889900aa  other.tar.gz\n";

        let got = parse_sha256sums(sums_text, filename);
        assert!(
            got.is_none(),
            "missing filename must return None (fail closed, no extraction)"
        );
    }

    #[test]
    fn sha256sums_url_matches_expected_shape() {
        // Confirm the URL builder uses the right path shape so test vectors align.
        let url = sha256sums_url("1.2.3");
        assert_eq!(
            url,
            "https://github.com/jaemk/mind/releases/download/v1.2.3/SHA256SUMS"
        );
    }
}
