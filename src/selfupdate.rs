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

/// Decide whether the running binary needs replacing: up to date when the current
/// version already satisfies `>= target`, otherwise an update is pending.
// spec: CLI-140
pub fn decision(current: &str, target: &str) -> Decision {
    if version_at_least(current, target) {
        Decision::UpToDate
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
    }
}

/// `mind evolve [--check] [--yes] [--version <v>]` — update the running binary.
///
/// `--version` resolves the target WITHOUT any network call, so
/// `evolve --check --version <v>` is fully offline. With no `--version`, the
/// latest release is fetched from the GitHub API. `--check` reports the decision
/// and returns without downloading. Otherwise, unless `--yes`, it prompts before
/// replacing the binary.
pub fn run(check: bool, yes: bool, version: Option<String>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    // Resolve (and validate) the platform target up front, so an unsupported
    // platform fails before any network call.
    let target = target_triple(os, arch)?;

    // Resolve the target version: an explicit --version bypasses the network
    // entirely; otherwise fetch and parse the latest release tag.
    let target_version = match version {
        Some(v) => v.strip_prefix('v').unwrap_or(&v).to_string(),
        None => {
            let json = fetch_to_string(&latest_release_api())?;
            parse_latest_tag(&json)?
        }
    };

    let decision = decision(current, &target_version);
    let out = crate::render::ctx();

    if check {
        // CLI-141: report and change nothing, without downloading.
        if out.json {
            let outcome = match decision {
                Decision::UpToDate => "up-to-date",
                Decision::Update => "available",
            };
            return print_evolve_json(&target_version, outcome);
        }
        let marker = match decision {
            Decision::UpToDate => out.ok(),
            Decision::Update => out.warn(),
        };
        println!(
            "{marker} {}",
            check_report(current, &target_version, &decision)
        );
        return Ok(());
    }

    if decision == Decision::UpToDate {
        if out.json {
            return print_evolve_json(&target_version, "up-to-date");
        }
        println!("{} mind {current} is already up to date", out.ok());
        return Ok(());
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
    let value = serde_json::json!({
        "action": "evolve",
        "target": version,
        "outcome": outcome,
    });
    let s = serde_json::to_string_pretty(&value).map_err(|e| MindError::json("json output", e))?;
    println!("{s}");
    Ok(())
}

/// Download the release archive, extract it, and atomically swap the new binary
/// for the running executable. Imperative and network-touching; the swap is
/// atomic so any failure leaves the existing binary intact.
fn download_and_swap(url: &str, current: &str, target_version: &str) -> Result<()> {
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
    fetch_to_file(url, &archive)?;

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

/// Atomically replace `current_exe` with `new_bin`: copy the new binary to a temp
/// file in the SAME directory as the running executable (so the rename stays on
/// one filesystem), make it executable, then rename it over the target. A rename
/// or permission failure on a non-writable target is reported as
/// `TargetNotWritable`.
fn swap_in_place(new_bin: &Path, current_exe: &Path) -> Result<()> {
    let dir = current_exe
        .parent()
        .ok_or_else(|| MindError::TargetNotWritable {
            path: current_exe.display().to_string(),
        })?;
    let staged = dir.join(".mind-update.tmp");

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

/// Create a unique temp directory for the download. Uses the system temp dir; the
/// caller removes it.
fn mktemp_dir() -> Result<std::path::PathBuf> {
    let base = std::env::temp_dir().join(format!("mind-evolve-{}", std::process::id()));
    std::fs::create_dir_all(&base).map_err(|e| MindError::io(&base, e))?;
    Ok(base)
}

/// Fetch a URL to a string via curl or wget, mirroring install.sh's secure flags.
fn fetch_to_string(url: &str) -> Result<String> {
    let output = if have("curl") {
        Command::new("curl")
            .args([
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                url,
            ])
            .output()
            .map_err(|e| MindError::io("curl", e))?
    } else if have("wget") {
        Command::new("wget")
            .args(["--https-only", "-qO-", url])
            .output()
            .map_err(|e| MindError::io("wget", e))?
    } else {
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: "need curl or wget on PATH".to_string(),
        });
    };
    if !output.status.success() {
        return Err(MindError::DownloadFailed {
            url: url.to_string(),
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fetch a URL to a file via curl or wget, mirroring install.sh's secure flags.
fn fetch_to_file(url: &str, dest: &Path) -> Result<()> {
    let status = if have("curl") {
        Command::new("curl")
            .args([
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                url,
                "-o",
            ])
            .arg(dest)
            .status()
            .map_err(|e| MindError::io("curl", e))?
    } else if have("wget") {
        Command::new("wget")
            .args(["--https-only", "-qO"])
            .arg(dest)
            .arg(url)
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
        // current == target => up to date.
        assert_eq!(decision("0.3.0", "0.3.0"), Decision::UpToDate);
        // target newer => update.
        assert_eq!(decision("0.2.0", "0.3.0"), Decision::Update);
        // current newer => up to date (never a downgrade).
        assert_eq!(decision("0.4.0", "0.3.0"), Decision::UpToDate);
    }

    #[test]
    // spec: CLI-141
    fn check_report_reflects_the_decision_without_network() {
        // The --check branch reports pending vs up-to-date purely from the
        // decision over an explicit target version: no network is consulted.
        let pending = decision("0.2.0", "0.3.0");
        assert_eq!(pending, Decision::Update);
        let report = check_report("0.2.0", "0.3.0", &pending);
        assert!(report.contains("0.2.0"), "report: {report}");
        assert!(report.contains("0.3.0"), "report: {report}");
        assert!(report.contains("available"), "report: {report}");

        let current = decision("0.3.0", "0.3.0");
        assert_eq!(current, Decision::UpToDate);
        let report = check_report("0.3.0", "0.3.0", &current);
        assert!(report.contains("up to date"), "report: {report}");
    }
}
