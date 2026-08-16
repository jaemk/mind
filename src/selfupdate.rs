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
        // Linux resolves to the statically linked musl artifact, matching
        // resources/install.sh: the gnu build carries the glibc floor of the
        // runner that produced it, so it fails to start on older
        // distributions after the download has already reported success.
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
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
        // A prerelease `current` (e.g. `0.23.1-dev`, a prerelease version
        // string such as one from a fork or a packager -- this repo's own
        // releases never carry one, see CARGO_PKG_VERSION in `run`) has the
        // same NUMERIC view as its base release `0.23.1`, so `version_at_least`
        // reads them as equal in both directions. But a prerelease predates
        // its base release, so treat a prerelease current as strictly below a
        // release target of the same numeric version: offer the update
        // (including an explicit `--to 0.23.1` onto its own base) rather than
        // claiming up-to-date. The `version_at_least(target, current)` guard
        // restricts this to a numeric TIE, so a genuinely newer prerelease
        // (`0.24.0-dev` vs release `0.23.1`) is unaffected.
        if is_prerelease(current) && !is_prerelease(target) && version_at_least(target, current) {
            return Decision::Update;
        }
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

/// Whether `v` carries a prerelease suffix (a `-` segment, e.g. `-dev` or
/// `-rc.2`, such as a fork or packager might append). Build metadata (`+...`)
/// is not a prerelease. Used by [`decision`] to break a numeric tie between a
/// prerelease and its base release.
fn is_prerelease(v: &str) -> bool {
    v.split_once('+').map_or(v, |(base, _)| base).contains('-')
}

/// The one-line status `--check` (and the run path) reports: the running version,
/// the target, and whether an update is pending. Pure so it is unit-testable
/// without touching the network.
///
/// `triple` is the resolved release target triple (`target_triple`) -- the exact
/// artifact `evolve` would fetch. It is appended as a trailing `-- target
/// <triple>` clause so the wording of each existing branch stays byte-for-byte
/// intact as a prefix (STO-65): the release artifacts recently changed from gnu
/// to musl on Linux, so surfacing which artifact is about to be downloaded,
/// before anything is fetched, lets a stale-glibc concern be caught at `--check`
/// time instead of after the swap.
// spec: CLI-141 STO-65
fn check_report(current: &str, target: &str, decision: &Decision, triple: &str) -> String {
    match decision {
        Decision::UpToDate => {
            format!("mind {current} is up to date (latest is {target}) -- target {triple}")
        }
        Decision::Update => {
            format!(
                "mind {current} -> {target} available; run `mind evolve` to update -- target {triple}"
            )
        }
        // spec: CLI-147
        Decision::PinnedBelowCurrent => {
            format!(
                "pinned {target} is below the running {current}; not downgrading -- target {triple}"
            )
        }
    }
}

/// Consult the managed policy for the self-update control (POL-51..POL-54).
///
/// Returns:
/// - `Ok(None)` when the policy allows `evolve` to any version (no pin).
/// - `Ok(Some(pin))` when the policy pins to a specific version (use as `--to`).
/// - `Err(SelfUpdatePolicy)` when `evolve` is disabled (POL-52) or when
///   `user_version` conflicts with the pin (POL-53).
///
/// Pure: no network call. `user_version` is the raw `--to` argument (`--version`
/// is a hidden deprecated alias for the same value; may have a leading `v`,
/// which is stripped before comparison).
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
                             --to {uv_clean} conflicts with the pin"
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

    // spec: STO-76 -- validate the resolved target version BEFORE it is
    // interpolated into either URL it drives (the release asset URL and the
    // SHA256SUMS URL, both built later from this same string). A value
    // carrying path segments (e.g. an explicit `--to
    // "1/../../../../attacker/mind/releases/download/v1"`, or a compromised
    // release's `tag_name`) would otherwise re-point both URLs at an
    // attacker-controlled location -- curl normalizes `..` segments -- so the
    // SHA256SUMS digest check would silently compare the attacker's binary
    // against the attacker's own digest file. Reject before any URL is built
    // and before any download-step network call.
    //
    // This uses `is_plausible_release_tag`, NOT `is_plausible_version`: the
    // two answer different questions (see `is_plausible_release_tag`'s doc).
    // A release tag legitimately carries a semver prerelease/build suffix
    // (`evolve --to 1.2.3-rc1` is the only way to reach a prerelease, since
    // `releases/latest` never surfaces one), so the validator here accepts
    // that shape while still rejecting anything that could escape or split a
    // URL path segment.
    if !crate::mindfile::is_plausible_release_tag(&target_version) {
        return Err(MindError::SelfUpdatePolicy {
            detail: format!(
                "self-update target version {target_version:?} is not a plausible \
                 release tag (e.g. \"1.2.3\" or \"1.2.3-rc1\"); refusing before \
                 building the release URL"
            ),
        });
    }

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
            return print_evolve_json(&target_version, outcome, target);
        }
        let marker = match d {
            Decision::UpToDate | Decision::PinnedBelowCurrent => out.ok(),
            Decision::Update => out.warn(),
        };
        println!(
            "{marker} {}",
            check_report(current, &target_version, &d, target)
        );
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
                return print_evolve_json(&target_version, "up-to-date", target);
            }
            println!("{} mind {current} is already up to date", out.ok());
            return Ok(());
        }
        // spec: CLI-147 -- explicit pin below running version: report and exit 0,
        // do NOT download or replace the binary.
        Decision::PinnedBelowCurrent => {
            if out.json {
                return print_evolve_json(&target_version, "not-downgrading", target);
            }
            println!(
                "{} {}",
                out.ok(),
                check_report(current, &target_version, &d, target)
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

    if !yes {
        // spec: LIFE-45 -- B1: `--json` is non-interactive, mirroring DEP-60: a
        // non-TTY run or a `--json` run without `--yes` refuses rather than
        // swapping the binary unprompted.
        if !crate::hook::is_tty() || out.json {
            return Err(MindError::ConfirmationRequired {
                action: format!("updating mind to {target_version}"),
            });
        }
        if !crate::commands::confirm(&format!("update mind to {target_version}?"))? {
            println!("aborted; nothing changed");
            return Ok(());
        }
    }

    let url = asset_url(&target_version, target);
    download_and_swap(&url, current, &target_version, target)
}

/// Emit the structured `evolve` result (CLI-153) under `--json`.
///
/// `target_triple` adds the `target_triple` key (STO-65) alongside the existing
/// `action`/`target`/`outcome` keys, which are never renamed: `--json` consumers
/// already depend on those names.
fn print_evolve_json(version: &str, outcome: &str, target_triple: &str) -> Result<()> {
    crate::render::print_json(&serde_json::json!({
        "action": "evolve",
        "target": version,
        "target_triple": target_triple,
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

/// The result of a soft build-provenance verification attempt (STO-66).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttestationOutcome {
    /// `gh attestation verify` succeeded: the archive matches a valid, signed
    /// build-provenance attestation published by the release repo.
    Verified,
    /// `gh` could not even attempt the verification -- an old `gh` with no
    /// `attestation` subcommand, or a network-level failure reaching GitHub --
    /// which is not a statement about the artifact itself. Carries the
    /// (sanitized) reason for the human-readable note.
    ToolingError(String),
    /// `gh` ran the check and reported the artifact does NOT verify: no
    /// matching attestation, a signer/repo mismatch, or an explicit signature
    /// failure. Deliberately fail-closed (see `is_gh_tooling_error`):
    /// "no attestations found" is `gh`'s output both for "this release
    /// predates build-provenance attestations" and for "this artifact was
    /// substituted", and the two are not distinguishable from its output, so
    /// both must abort rather than silently trusting an unverified artifact.
    GenuineFailure(String),
}

/// The `gh` argv that verifies a downloaded archive's build-provenance
/// attestation against the release repo (STO-66), matching
/// `resources/install.sh`'s `gh attestation verify "$tmp/$asset" --repo "$REPO"`.
/// Pure (no I/O) so it is unit-testable without spawning a process.
pub(crate) fn gh_attestation_verify_args(archive_path: &str, repo: &str) -> Vec<String> {
    vec![
        "attestation".into(),
        "verify".into(),
        archive_path.into(),
        "--repo".into(),
        repo.into(),
    ]
}

/// Classify `gh attestation verify`'s (sanitized) stderr as a TOOLING problem --
/// `gh` itself could not run the check -- rather than a verification result
/// (STO-66).
///
/// Deliberately narrow: anything not matched here is treated as a genuine
/// verification failure and aborts the swap. The interesting failure mode (a
/// substituted artifact has no valid attestation) surfaces through the exact
/// same "no attestations found" / HTTP-404 wording that `gh` also uses for a
/// merely-absent attestation (confirmed empirically: querying a real, unmodified
/// artifact's digest under the wrong repo, and querying a tampered artifact's
/// digest under the correct repo, both produce the same class of "not found"
/// message). There is no reliable way to tell those two cases apart from `gh`'s
/// output, so the ambiguity fails CLOSED here rather than silently passing an
/// unverifiable artifact through.
pub(crate) fn is_gh_tooling_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    const TOOLING_MARKERS: &[&str] = &[
        // Old `gh` with no `attestation verify` subcommand/flag.
        "unknown command",
        "unknown flag",
        // Network-level failures reaching the GitHub API -- `gh` never got an
        // answer to verify against, so this says nothing about the artifact.
        "dial tcp",
        "no such host",
        "connection refused",
        "i/o timeout",
        "context deadline exceeded",
        "tls handshake",
        "certificate signed by unknown authority",
        // Auth required (e.g. a private repo, or an org policy) -- `gh` could
        // not even ask the question.
        "gh auth login",
    ];
    TOOLING_MARKERS.iter().any(|m| lower.contains(m))
}

/// Soft-verify a downloaded archive's build-provenance attestation (STO-66),
/// mirroring `resources/install.sh`'s `gh attestation verify` step.
///
/// `gh_cmd` is the command name to invoke: `"gh"` in production. Tests point it
/// at a name that resolves to nothing (simulating "`gh` absent" deterministically,
/// without needing to hide the real `gh` from PATH) or at a fake script placed
/// first on PATH (simulating a specific `gh` outcome), mirroring the fake-curl
/// pattern used elsewhere in this file.
///
/// Returns `None` when `gh_cmd` is not on PATH: evolve proceeds silently,
/// exactly like install.sh's `if command -v gh` gate (no note printed for a
/// plain absence, only for a `gh` that ran but could not complete the check).
fn attestation_step(gh_cmd: &str, archive: &Path) -> Option<AttestationOutcome> {
    if !have(gh_cmd) {
        return None;
    }
    let archive_str = archive.to_string_lossy();
    let args = gh_attestation_verify_args(&archive_str, REPO);
    let output = match Command::new(gh_cmd).args(&args).output() {
        Ok(o) => o,
        // A spawn failure after `have()` reported present (e.g. a TOCTOU race,
        // or a `gh` that is on PATH but not executable) is itself a tooling
        // problem, not a statement about the artifact.
        Err(e) => return Some(AttestationOutcome::ToolingError(e.to_string())),
    };
    if output.status.success() {
        return Some(AttestationOutcome::Verified);
    }
    let stderr = crate::sanitize::strip_ansi(String::from_utf8_lossy(&output.stderr).trim());
    if is_gh_tooling_error(&stderr) {
        Some(AttestationOutcome::ToolingError(stderr))
    } else {
        Some(AttestationOutcome::GenuineFailure(stderr))
    }
}

/// Download the release archive, extract it, and atomically swap the new binary
/// for the running executable. Imperative and network-touching; the swap is
/// atomic so any failure leaves the existing binary intact.
///
/// Holds the global exclusive lock (STO-46) for the entire download-and-swap
/// step so two concurrent `mind evolve` invocations cannot race.
fn download_and_swap(
    url: &str,
    current: &str,
    target_version: &str,
    target_triple: &str,
) -> Result<()> {
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

    // spec: STO-66 -- soft-verify the archive's build-provenance attestation
    // when `gh` is available, before extraction/swap. Absent `gh`, or a
    // tooling error, proceeds with (at most) a note; a genuine verification
    // failure aborts before anything is extracted or swapped.
    match attestation_step("gh", &archive) {
        None => {}
        Some(AttestationOutcome::Verified) => {
            if !out.json {
                println!("{} build provenance verified", out.ok());
            }
        }
        Some(AttestationOutcome::ToolingError(reason)) => {
            if !out.json {
                println!(
                    "{} build provenance could not be verified ({reason}); continuing",
                    out.warn()
                );
            }
        }
        Some(AttestationOutcome::GenuineFailure(reason)) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(MindError::AttestationVerificationFailed { reason });
        }
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
        return print_evolve_json(target_version, "updated", target_triple);
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
    mktemp_dir_prefixed("mind-evolve")
}

/// Like `mktemp_dir`, but with a caller-chosen name prefix instead of the
/// hardcoded `mind-evolve` used by the download/auth-config paths. Shared with
/// `tui::action`'s stdout-capture file (TUI-61), which needs the identical
/// exclusive-create + 0700 scheme but under its own `mind-tui-capture` prefix so
/// the two features' temp dirs stay visually distinguishable on disk.
// spec: TUI-61
pub(crate) fn mktemp_dir_prefixed(prefix: &str) -> Result<std::path::PathBuf> {
    let pid = std::process::id();
    let seq = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let base = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{seq}"));
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

/// Whether `token` is safe to embed in a curl `--config` file (STO-61) or a
/// `Authorization:` header value.
///
/// Rejects:
/// - any control character (including `\n`/`\r`): the config file is
///   `key = "value"` lines, one directive per line, so an embedded newline lets
///   the token inject additional curl directives (e.g. `output = ...` or
///   `url = ...`) that curl will honor -- B10.
/// - `"` and `\`: curl's config quoted-string syntax does not get escaping
///   applied when the token is interpolated in, so either character could
///   break out of the quoted value -- C20.
///
/// Pure (no I/O) so it is unit-testable without touching the environment.
// spec: STO-62
pub(crate) fn is_safe_token(token: &str) -> bool {
    !token.chars().any(|c| c.is_control()) && !token.contains('"') && !token.contains('\\')
}

/// A GitHub token from the environment, if set AND safe to use: `GITHUB_TOKEN`
/// first, then `GH_TOKEN` (matching the `gh` CLI), first non-empty *and safe*
/// wins. Trailing whitespace is trimmed so a token read from a file with a
/// trailing newline still forms a valid header.
///
/// A candidate that fails `is_safe_token` (B10: an embedded control character,
/// `"`, or `\`) is skipped rather than used, with a warning on stderr -- evolve
/// fails CLOSED (no auth header) instead of forwarding an unsafe value into the
/// curl config file or an HTTP header. This is deliberately non-fatal: a
/// malformed `GITHUB_TOKEN` (e.g. a CI expression that expanded wrong) should
/// degrade the request to unauthenticated, not abort the whole `evolve`.
// spec: STO-62
fn github_token() -> Option<String> {
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            if !is_safe_token(v) {
                eprintln!(
                    "warning: ${var} contains characters unsafe for evolve's authenticated \
                     GitHub API request (a control character, '\"', or '\\'); proceeding \
                     without authentication"
                );
                continue;
            }
            return Some(v.to_string());
        }
    }
    None
}

/// The curl auth config-file content authenticating a GitHub REST API request
/// (STO-61).
///
/// Returns `Some("header = \"Authorization: Bearer <token>\"\n")` only when a
/// non-empty token is present AND the URL targets `api.github.com` (the same
/// host gating as before, STO-57), so the token is never forwarded to the
/// artifact CDN on a cross-host redirect. The header is delivered to curl via a
/// 0600 `--config` file rather than on argv (STO-61), so it is not exposed in
/// `/proc/<pid>/cmdline` to a local co-tenant during the brief API call. Pure
/// (token passed in) so it is unit-testable without touching the environment.
// spec: STO-61
pub(crate) fn curl_auth_config_content(url: &str, token: Option<&str>) -> Option<String> {
    match token {
        Some(t) if !t.is_empty() && is_github_api_url(url) => {
            Some(format!("header = \"Authorization: Bearer {t}\"\n"))
        }
        _ => None,
    }
}

/// The extra curl args pointing at the auth config file (STO-61).
///
/// When an auth config file was written (a token present AND the URL targets
/// `api.github.com`; see `curl_auth_config_content`), returns `--config <path>`;
/// otherwise empty. The token itself never appears here, so it stays off argv.
/// Pure (path passed in) so the arg vector is unit-testable.
// spec: STO-61
pub(crate) fn curl_auth_args(config_path: Option<&str>) -> Vec<String> {
    match config_path {
        Some(p) => vec!["--config".into(), p.into()],
        None => vec![],
    }
}

/// Write the curl auth config (STO-61) to a 0600-mode file inside a fresh 0700
/// temp directory, returning the file path. The caller removes it (best-effort)
/// after the fetch via `remove_curl_auth_config`. The file is created write-only
/// to the owner (`create_new` + `mode(0o600)`) so the bearer token it carries is
/// never group- or world-readable while curl reads it.
fn write_curl_auth_config(content: &str) -> Result<std::path::PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let dir = mktemp_dir()?;
    let path = dir.join("curl-auth.cfg");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| MindError::io(&path, e))?;
    f.write_all(content.as_bytes())
        .map_err(|e| MindError::io(&path, e))?;
    Ok(path)
}

/// `write_curl_auth_config`, degraded (C19): a failure to write the temp auth
/// config file (a read-only or full `TMPDIR`, e.g.) warns on stderr and
/// returns `None` instead of propagating, so `evolve` falls back to an
/// unauthenticated request rather than hard-failing outright. The
/// unauthenticated request still works below GitHub's per-IP rate limit, so
/// this is strictly better than aborting.
// spec: STO-62
fn write_curl_auth_config_or_warn(content: &str) -> Option<std::path::PathBuf> {
    match write_curl_auth_config(content) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!(
                "warning: could not write the temporary auth config file for the \
                 authenticated GitHub API request ({e}); proceeding without authentication"
            );
            None
        }
    }
}

/// Best-effort removal of the auth config file and its temp directory (STO-61),
/// so the token-bearing file does not linger after the API call.
fn remove_curl_auth_config(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
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
        // spec: STO-61 STO-62 -- pass the bearer token via a 0600 --config file,
        // never argv; a write failure degrades to unauthenticated (C19) rather
        // than failing the whole fetch.
        let auth_cfg = curl_auth_config_content(url, token.as_deref())
            .and_then(|content| write_curl_auth_config_or_warn(&content));
        if let Some(ref p) = auth_cfg {
            args.extend(curl_auth_args(Some(&p.to_string_lossy())));
        }
        let result = Command::new("curl")
            .args(args)
            .output()
            .map_err(|e| MindError::io("curl", e));
        if let Some(ref p) = auth_cfg {
            remove_curl_auth_config(p);
        }
        result?
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
        // spec: STO-61 STO-62 -- pass the bearer token via a 0600 --config file,
        // never argv; a write failure degrades to unauthenticated (C19) rather
        // than failing the whole fetch.
        let auth_cfg = curl_auth_config_content(url, token.as_deref())
            .and_then(|content| write_curl_auth_config_or_warn(&content));
        if let Some(ref p) = auth_cfg {
            args.extend(curl_auth_args(Some(&p.to_string_lossy())));
        }
        let result = Command::new("curl")
            .args(args)
            .status()
            .map_err(|e| MindError::io("curl", e));
        if let Some(ref p) = auth_cfg {
            remove_curl_auth_config(p);
        }
        result?
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

    // ---- STO-57 / STO-61: GitHub API auth header -----------------------------

    #[test]
    // spec: STO-57 STO-61
    fn auth_config_content_for_api_host_and_wget_header() {
        // A token present + an api.github.com URL -> curl gets config-file content
        // carrying the bearer header, and wget still gets the inline --header arg.
        let url = "https://api.github.com/repos/jaemk/mind/releases/latest";
        let content = curl_auth_config_content(url, Some("tok123"))
            .expect("curl config content must be produced on the API host");
        assert_eq!(
            content, "header = \"Authorization: Bearer tok123\"\n",
            "curl config content must carry the bearer header in curl config syntax: {content:?}"
        );
        let wget = wget_auth_args(url, Some("tok123"));
        assert_eq!(
            wget,
            vec!["--header=Authorization: Bearer tok123".to_string()],
            "wget must send the bearer header on the API host (argv form): {wget:?}"
        );
    }

    #[test]
    // spec: STO-57 STO-61
    fn auth_never_leaks_token_to_non_api_hosts() {
        // The token must NOT be attached to the artifact CDN download, so it is
        // not forwarded across a cross-host redirect.
        let url = "https://github.com/jaemk/mind/releases/download/v1.2.3/mind-1.2.3-x.tar.gz";
        assert!(
            curl_auth_config_content(url, Some("tok123")).is_none(),
            "curl must not build an auth config for github.com"
        );
        assert!(
            wget_auth_args(url, Some("tok123")).is_empty(),
            "wget must not send a token to github.com"
        );
    }

    #[test]
    // spec: STO-57 STO-61
    fn auth_empty_without_a_token() {
        // No token (or an empty one) -> the request is byte-for-byte unchanged.
        let url = "https://api.github.com/repos/jaemk/mind/releases/latest";
        assert!(
            curl_auth_config_content(url, None).is_none(),
            "no token -> no curl auth config"
        );
        assert!(
            wget_auth_args(url, None).is_empty(),
            "no token -> no wget header"
        );
        assert!(
            curl_auth_config_content(url, Some("")).is_none(),
            "empty token -> no curl auth config"
        );
        assert!(
            wget_auth_args(url, Some("")).is_empty(),
            "empty token -> no wget header"
        );
    }

    #[test]
    // spec: STO-62
    fn is_safe_token_rejects_control_chars_and_quote_backslash() {
        // A clean token is safe.
        assert!(is_safe_token("ghp_abcDEF1234567890"));
        // An embedded newline could inject an extra curl config directive
        // (B10): reject.
        assert!(!is_safe_token("tok123\noutput = /tmp/pwned"));
        assert!(!is_safe_token("tok123\r\nurl = https://evil.example/"));
        // Any other control character (e.g. a bare CR or a NUL byte) is rejected too.
        assert!(!is_safe_token("tok\x00123"));
        assert!(!is_safe_token("tok\x1b[31m123"));
        // `"` and `\` are not escaped by the config file's quoted-string syntax
        // (C20): reject either.
        assert!(!is_safe_token("tok\"123"));
        assert!(!is_safe_token("tok\\123"));
        // Empty string has no unsafe characters, so it is "safe" by this
        // predicate; callers gate on non-empty separately.
        assert!(is_safe_token(""));
    }

    #[test]
    // spec: STO-62
    fn github_token_rejects_unsafe_env_value_and_falls_back() {
        // spec: STO-62 -- an unsafe GITHUB_TOKEN is skipped (fail closed) and
        // GH_TOKEN is tried next; with neither safe, the result is None (no
        // auth), never a propagated error.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_gh = std::env::var("GITHUB_TOKEN").ok();
        let orig_gh2 = std::env::var("GH_TOKEN").ok();

        // SAFETY: ENV_LOCK (the crate-wide shared lock, C18) is held for the
        // duration of every mutation and read below.
        unsafe {
            std::env::set_var("GITHUB_TOKEN", "tok123\noutput = /tmp/pwned");
            std::env::remove_var("GH_TOKEN");
        }
        assert_eq!(
            github_token(),
            None,
            "an unsafe GITHUB_TOKEN with no valid fallback must yield no token, not an error"
        );

        unsafe {
            std::env::set_var("GITHUB_TOKEN", "tok123\noutput = /tmp/pwned");
            std::env::set_var("GH_TOKEN", "goodtoken456");
        }
        assert_eq!(
            github_token(),
            Some("goodtoken456".to_string()),
            "an unsafe GITHUB_TOKEN must fall through to a safe GH_TOKEN"
        );

        unsafe {
            std::env::set_var("GITHUB_TOKEN", "cleantoken789");
            std::env::remove_var("GH_TOKEN");
        }
        assert_eq!(
            github_token(),
            Some("cleantoken789".to_string()),
            "a safe token must still be returned unchanged"
        );

        // Restore.
        unsafe {
            match orig_gh {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
            match orig_gh2 {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
        }
        drop(guard);
    }

    #[test]
    // spec: STO-62
    fn write_curl_auth_config_or_warn_degrades_on_write_failure() {
        // C19 -- when the auth config file cannot be written (simulated here by
        // pointing content-writing at a path that cannot exist: a parent that is
        // actually a regular file, so `create_dir` inside `mktemp_dir` fails
        // because TMPDIR itself is unusable), the degraded wrapper must return
        // None (fail OPEN to an unauthenticated request) rather than the caller
        // propagating a hard error.
        //
        // We cannot easily force TMPDIR-level failures hermetically without
        // mutating global env (which every other evolve test also touches), so
        // this test drives the always-failing branch directly via a full
        // TMPDIR override, restoring it immediately after.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_tmpdir = std::env::var("TMPDIR").ok();

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let blocked_path = std::env::temp_dir().join(format!(
            "mind-evolve-blocked-tmpdir-{}-{n}",
            std::process::id()
        ));
        // Create a REGULAR FILE at the path we are about to point TMPDIR at, so
        // `create_dir` for any path under it fails with NotADirectory/Exists.
        std::fs::write(&blocked_path, b"not a directory").expect("seed blocking file");

        // SAFETY: ENV_LOCK is held for the duration of this mutation and the
        // call below.
        unsafe {
            std::env::set_var("TMPDIR", &blocked_path);
        }

        let result = write_curl_auth_config_or_warn("header = \"Authorization: Bearer x\"\n");

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match &orig_tmpdir {
                Some(v) => std::env::set_var("TMPDIR", v),
                None => std::env::remove_var("TMPDIR"),
            }
        }
        drop(guard);
        let _ = std::fs::remove_file(&blocked_path);

        assert!(
            result.is_none(),
            "a temp-dir write failure must degrade to None (unauthenticated), not panic/error: {result:?}"
        );
    }

    #[test]
    // spec: STO-61
    fn curl_argv_uses_config_flag_and_never_carries_the_token() {
        // The built curl arg vector references the auth config file via --config
        // and never contains the token or an -H Authorization header on argv, so
        // /proc/<pid>/cmdline cannot leak the token.
        let path = "/run/mind-evolve-x/curl-auth.cfg";
        let mut args = curl_string_args(
            "https://api.github.com/repos/jaemk/mind/releases/latest",
            15,
        );
        args.extend(curl_auth_args(Some(path)));
        let cfg = args
            .iter()
            .position(|a| a == "--config")
            .expect("curl args must include --config");
        assert_eq!(
            args[cfg + 1],
            path,
            "the config-file path must follow --config"
        );
        assert!(
            !args.iter().any(|a| a.contains("tok123")),
            "the token must never appear on curl argv: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "-H" || a.starts_with("Authorization:")),
            "curl argv must not carry an -H Authorization header: {args:?}"
        );
        // No auth config path -> no --config arg at all.
        assert!(
            curl_auth_args(None).is_empty(),
            "no config path -> no --config arg"
        );
    }

    #[test]
    // spec: STO-61
    fn curl_auth_config_file_is_owner_only_0600() {
        // The written auth config file must be mode 0600 (owner read/write only)
        // so the bearer token it holds is not group- or world-readable.
        let content = curl_auth_config_content(
            "https://api.github.com/repos/jaemk/mind/releases/latest",
            Some("tok123"),
        )
        .expect("content must be produced");
        let path = write_curl_auth_config(&content).expect("must write the auth config");
        let meta = std::fs::metadata(&path).expect("config file must exist");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "auth config file must be mode 0600, got {mode:o}"
        );
        // spec: STO-61 -- the config file lives inside a FRESH 0700 temp
        // directory (`mktemp_dir`), not merely a 0600 file dropped into a
        // shared/world-traversable temp dir: without this, `remove_curl_auth_config`
        // wiping only the file (not the dir) would still be tested, but the 0700
        // isolation of the directory itself would go unverified.
        let dir = path.parent().expect("config file must have a parent dir");
        let dir_meta = std::fs::metadata(dir).expect("temp dir must exist");
        assert!(
            dir_meta.is_dir(),
            "the auth config's parent must be a directory"
        );
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "the auth config's temp directory must be mode 0700, got {dir_mode:o}"
        );
        // The file content carries the header (curl reads it), proving the token
        // lives in the 0600 file rather than on argv.
        let on_disk = std::fs::read_to_string(&path).expect("must read config back");
        assert!(
            on_disk.contains("Authorization: Bearer tok123"),
            "the auth config file must carry the bearer header: {on_disk:?}"
        );
        remove_curl_auth_config(&path);
        assert!(
            !path.exists(),
            "remove_curl_auth_config must delete the file"
        );
        assert!(
            !dir.exists(),
            "remove_curl_auth_config must also remove the temp directory"
        );
    }

    #[test]
    // spec: STO-61
    fn curl_file_argv_uses_config_flag_and_never_carries_the_token() {
        // Mirrors `curl_argv_uses_config_flag_and_never_carries_the_token` but
        // for the fetch_to_file arg builder (`curl_file_args`), so the
        // file-download path (used for the release archive and SHA256SUMS,
        // `download_and_swap`) is covered too, not just the string-fetch path
        // (used for the GitHub API JSON response).
        let path = "/run/mind-evolve-y/curl-auth.cfg";
        let mut args = curl_file_args(
            "https://api.github.com/repos/jaemk/mind/releases/latest",
            "/tmp/dest-file",
            15,
        );
        args.extend(curl_auth_args(Some(path)));
        let cfg = args
            .iter()
            .position(|a| a == "--config")
            .expect("curl file-fetch args must include --config");
        assert_eq!(
            args[cfg + 1],
            path,
            "the config-file path must follow --config"
        );
        assert!(
            !args.iter().any(|a| a.contains("tok123")),
            "the token must never appear on curl file-fetch argv: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "-H" || a.starts_with("Authorization:")),
            "curl file-fetch argv must not carry an -H Authorization header: {args:?}"
        );
    }

    /// Serializes tests that mutate process-wide env vars (`PATH`,
    /// `GITHUB_TOKEN`/`GH_TOKEN`) so they don't race each other. This is the
    /// crate-wide lock defined in `src/paths.rs` (C18): `set_var`/`remove_var`
    /// soundness is process-wide, not module-wide, and `src/paths.rs` and
    /// `src/commands.rs` mutate their own overlapping set of env vars
    /// (`MIND_AGENT_HOMES`, `MIND_POLICY_FILE`, `MIND_HOME`, `CLAUDE_HOME`, ...)
    /// in the same multithreaded test binary, some via a spawned `git`, which
    /// snapshots `environ`. Using one shared lock here instead of a
    /// module-local one closes that gap.
    use crate::paths::ENV_LOCK;

    /// Write an executable fake `curl` at `dir/curl` that records its argv (one
    /// arg per line) to `capture_path` and always exits non-zero (simulating a
    /// curl failure) without touching the network.
    fn write_fake_failing_curl(dir: &Path, capture_path: &Path) {
        let script_path = dir.join("curl");
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {:?}\nexit 7\n",
            capture_path
        );
        std::fs::write(&script_path, script).expect("write fake curl");
        let mut perms = std::fs::metadata(&script_path)
            .expect("stat fake curl")
            .permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script_path, perms).expect("chmod fake curl");
    }

    #[test]
    // spec: STO-61
    fn fetch_to_string_removes_auth_config_file_even_when_curl_fails() {
        // The auth config file is written before invoking curl and removed
        // right after (`remove_curl_auth_config`), unconditionally on the
        // Result -- i.e. even when curl itself fails. This drives the real
        // `fetch_to_string` private function against a fake, always-failing
        // `curl` on PATH (never touching the network) and verifies (a) the
        // fetch reports failure, (b) the config file curl was pointed at via
        // `--config` no longer exists afterward, and (c) the token never
        // appeared on curl's argv.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let scratch =
            std::env::temp_dir().join(format!("mind-evolve-fake-curl-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let capture_path = scratch.join("argv-capture.txt");
        write_fake_failing_curl(&scratch, &capture_path);

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let orig_gh_token = std::env::var("GITHUB_TOKEN").ok();
        let orig_gh_token2 = std::env::var("GH_TOKEN").ok();
        let new_path = format!("{}:{orig_path}", scratch.display());
        // SAFETY: ENV_LOCK is held for the duration of the mutation below, and
        // no other test in this module reads/writes PATH, GITHUB_TOKEN, or
        // GH_TOKEN, or spawns a real curl/wget process.
        unsafe {
            std::env::set_var("PATH", &new_path);
            std::env::set_var("GITHUB_TOKEN", "tok123");
            std::env::remove_var("GH_TOKEN");
        }

        let result = fetch_to_string("https://api.github.com/repos/jaemk/mind/releases/latest");

        // Restore env immediately, before any assertion can panic and leave
        // the process env corrupted for later tests.
        // SAFETY: ENV_LOCK is still held.
        unsafe {
            std::env::set_var("PATH", &orig_path);
            match orig_gh_token {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
            match orig_gh_token2 {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
        }
        drop(guard);

        assert!(
            result.is_err(),
            "the fake curl exits non-zero, so fetch_to_string must report failure"
        );

        let argv = std::fs::read_to_string(&capture_path).expect("read captured argv");
        let lines: Vec<&str> = argv.lines().collect();
        let cfg_idx = lines
            .iter()
            .position(|a| *a == "--config")
            .expect("curl must have been invoked with --config: {lines:?}");
        let cfg_path = std::path::PathBuf::from(lines[cfg_idx + 1]);
        assert!(
            !cfg_path.exists(),
            "the auth config file must be removed even though curl failed: {cfg_path:?}"
        );
        assert!(
            !lines.iter().any(|a| a.contains("tok123")),
            "the token must never appear on curl's argv: {lines:?}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    // spec: STO-76
    fn run_refuses_a_malicious_latest_tag_before_any_second_network_call() {
        // A stubbed GitHub "latest release" response whose tag_name carries
        // path segments (e.g. from a repo/release takeover, or a TLS-
        // intercepting proxy) must be refused by `is_plausible_version`
        // validation before `run` ever builds the release asset URL or the
        // SHA256SUMS URL from it, and before any second network call (the
        // asset/sums download) is attempted. Drives the real `run()` against a
        // fake curl on PATH that always answers the malicious JSON and counts
        // its own invocations, never touching the network.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "mind-evolve-fake-curl-latest-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let counter_path = scratch.join("call-count.txt");
        let script_path = scratch.join("curl");
        // Every invocation appends one byte to the counter file and answers
        // with a tag_name that, once the leading 'v' is stripped, contains
        // '..' path segments -- the shape that would re-point the release
        // asset URL AND the SHA256SUMS URL at an attacker-controlled path
        // once curl normalizes them.
        let script = format!(
            "#!/bin/sh\nprintf x >> {counter:?}\nprintf '%s' '{{\"tag_name\":\"v1/../../../../attacker/mind/releases/download/v1\"}}'\nexit 0\n",
            counter = counter_path
        );
        std::fs::write(&script_path, script).expect("write fake curl");
        let mut perms = std::fs::metadata(&script_path)
            .expect("stat fake curl")
            .permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script_path, perms).expect("chmod fake curl");

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        let new_path = format!("{}:{orig_path}", scratch.display());
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below; no real network is reached (PATH resolves
        // `curl` to the fake script above first, and `run` never gets far
        // enough to need MIND_HOME).
        unsafe {
            std::env::set_var("PATH", &new_path);
            std::env::remove_var("MIND_POLICY_FILE");
        }

        let result = run(false, true, None);

        // Restore env immediately, before any assertion can panic.
        // SAFETY: ENV_LOCK is still held.
        unsafe {
            std::env::set_var("PATH", &orig_path);
            match orig_policy_file {
                Some(v) => std::env::set_var("MIND_POLICY_FILE", v),
                None => std::env::remove_var("MIND_POLICY_FILE"),
            }
        }
        drop(guard);

        match result {
            Err(MindError::SelfUpdatePolicy { detail }) => {
                assert!(
                    detail.contains("not a plausible"),
                    "must explain the version is implausible: {detail}"
                );
                assert!(
                    detail.contains("attacker"),
                    "must name the rejected value: {detail}"
                );
            }
            other => panic!("expected a SelfUpdatePolicy refusal, got {other:?}"),
        }

        let call_count =
            std::fs::read_to_string(&counter_path).expect("fake curl must have been invoked");
        assert_eq!(
            call_count.len(),
            1,
            "curl must be invoked exactly once (the latest-release lookup); a second \
             invocation would mean evolve proceeded to build a URL from the unvalidated \
             target version: {call_count:?}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    // spec: STO-76
    fn run_refuses_an_explicit_to_target_with_path_segments_with_no_network_call() {
        // An explicit `--to` value bypasses the network entirely for target
        // resolution (no `latest` lookup), so a malicious value must still be
        // refused purely from local validation -- no PATH stub needed; if
        // `run` somehow tried to shell out despite there being no curl/wget
        // stub on PATH, it would fail with a `DownloadFailed`/`Io` error
        // instead of `SelfUpdatePolicy`, and the assertion below would catch
        // that misbehavior.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
        }

        let result = run(
            true,
            false,
            Some("1/../../../../attacker/mind/releases/download/v1".to_string()),
        );

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match orig_policy_file {
                Some(v) => std::env::set_var("MIND_POLICY_FILE", v),
                None => std::env::remove_var("MIND_POLICY_FILE"),
            }
        }
        drop(guard);

        match result {
            Err(MindError::SelfUpdatePolicy { detail }) => {
                assert!(
                    detail.contains("not a plausible"),
                    "must explain the version is implausible: {detail}"
                );
                assert!(
                    detail.contains("attacker"),
                    "must name the rejected --to value: {detail}"
                );
            }
            other => panic!("expected a SelfUpdatePolicy refusal, got {other:?}"),
        }
    }

    #[test]
    // spec: STO-76 CLI-140
    fn run_accepts_an_explicit_prerelease_to_target_and_reaches_decision_end_to_end() {
        // Before the STO-76 fix reused `is_plausible_version` (digits and dots
        // only) at this site, `evolve --to 1.2.3-rc1` was refused outright --
        // testing a prerelease before promoting it is legitimate, and since
        // GitHub's `releases/latest` never surfaces a prerelease, an explicit
        // `--to` is the only way to reach one. This drives the real `run()`
        // entry path (not `decision()` directly, which would mask a
        // regression at the validation site upstream of it): a prerelease
        // `--to` value must pass the STO-76 validation, reach `decision()`,
        // and (since "1.2.3-rc1" is numerically far above the running
        // version) come back `Decision::Update`. No network call is possible
        // here at all (no curl/wget stub is put on PATH), so reaching the
        // `!yes` confirmation gate -- rather than a `DownloadFailed`/`Io`
        // error from an attempted shell-out, or a `SelfUpdatePolicy`
        // "not a plausible" refusal -- proves both that validation accepted
        // the value AND that `decision()` classified it as an update,
        // end-to-end through production code.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        let orig_tty = std::env::var("MIND_TTY").ok();
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::set_var("MIND_TTY", "0");
        }

        let result = run(false, false, Some("1.2.3-rc1".to_string()));

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match orig_policy_file {
                Some(v) => std::env::set_var("MIND_POLICY_FILE", v),
                None => std::env::remove_var("MIND_POLICY_FILE"),
            }
            match orig_tty {
                Some(v) => std::env::set_var("MIND_TTY", v),
                None => std::env::remove_var("MIND_TTY"),
            }
        }
        drop(guard);

        match result {
            Err(MindError::ConfirmationRequired { action }) => {
                assert!(
                    action.contains("1.2.3-rc1"),
                    "must be prompting to update to the resolved prerelease target: {action}"
                );
            }
            other => panic!(
                "expected ConfirmationRequired (proving the prerelease target passed \
                 validation and reached an Update decision), got {other:?}"
            ),
        }
    }

    #[test]
    // spec: STO-76
    fn run_refuses_traversal_smuggled_inside_an_explicit_prerelease_suffix() {
        // A `-`-prefixed suffix of dots and slashes must not become an escape
        // hatch around the STO-76 URL-path-segment check just because a
        // legitimate prerelease suffix is now accepted. Exercised through the
        // real `run()` entry path with no curl/wget stub on PATH, so any
        // attempt to shell out before validation would surface as a
        // DownloadFailed/Io error instead, which the match below would catch.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
        }

        let result = run(true, false, Some("1.0.0-../..".to_string()));

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match orig_policy_file {
                Some(v) => std::env::set_var("MIND_POLICY_FILE", v),
                None => std::env::remove_var("MIND_POLICY_FILE"),
            }
        }
        drop(guard);

        match result {
            Err(MindError::SelfUpdatePolicy { detail }) => {
                assert!(
                    detail.contains("not a plausible"),
                    "must explain the version is implausible: {detail}"
                );
                assert!(
                    detail.contains("1.0.0-../.."),
                    "must name the rejected --to value: {detail}"
                );
            }
            other => panic!("expected a SelfUpdatePolicy refusal, got {other:?}"),
        }
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
                // `--to` is the canonical evolve flag (`--version` is a hidden
                // deprecated alias, commit 40c495f); the pin-conflict message
                // must name the flag `evolve --help` actually documents.
                assert!(
                    detail.contains("--to 0.15.0"),
                    "must name the canonical --to flag, not the hidden --version alias: {detail}"
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
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            target_triple("linux", "aarch64").unwrap(),
            "aarch64-unknown-linux-musl"
        );
        assert_eq!(
            target_triple("macos", "aarch64").unwrap(),
            "aarch64-apple-darwin"
        );
    }

    #[test]
    // spec: CLI-142 -- a platform with no published artifact is an error and
    // nothing is changed.
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
    // spec: CLI-142 -- the artifact name shape `mind-<version>-<target>.tar.gz`
    // is what resources/install.sh and the Homebrew formula resolve, so every
    // install path lands on the same binary.
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
    // spec: CLI-140
    fn decision_prerelease_current_updates_onto_its_base_release() {
        // A source-built `-dev` binary shares the numeric view of its base
        // release, so the comparator reads them as equal; but the dev build
        // predates the release, so `decision` must offer the update rather than
        // claim up-to-date -- both when resolving latest and for an explicit
        // `--to` onto the same base version.
        assert_eq!(decision("0.23.1-dev", "0.23.1", false), Decision::Update);
        assert_eq!(decision("0.23.1-dev", "0.23.1", true), Decision::Update);
        // A genuinely newer dev build is NOT downgraded onto an older release.
        assert_eq!(decision("0.24.0-dev", "0.23.1", false), Decision::UpToDate);
        assert_eq!(
            decision("0.24.0-dev", "0.23.1", true),
            Decision::PinnedBelowCurrent
        );
        // Two prereleases of the same base stay up-to-date (no release to move to).
        assert_eq!(
            decision("0.23.1-dev", "0.23.1-dev", false),
            Decision::UpToDate
        );
        // A release current is never treated as a prerelease.
        assert_eq!(decision("0.23.1", "0.23.1", false), Decision::UpToDate);
    }

    #[test]
    // spec: CLI-140
    fn decision_dotted_prerelease_current_updates_onto_its_base_release() {
        // A DOTTED prerelease suffix (semver's `-rc.2` form, as opposed to a
        // bare `-dev`) must be handled identically to `decision_prerelease_
        // current_updates_onto_its_base_release` above. Before the
        // `version_at_least` fix (mindfile.rs), per-dotted-component suffix
        // stripping mis-parsed "1.0.0-rc.2" as [1, 0, 0, 2] -- numerically
        // ABOVE the plain "1.0.0" it should tie with -- so `is_prerelease`
        // correctly flagged the value as a prerelease, but the numeric-tie
        // guard `version_at_least(target, current)` never held, and `decision`
        // fell through to UpToDate / PinnedBelowCurrent instead of offering
        // the update that supersedes the prerelease.
        assert_eq!(decision("1.0.0-rc.2", "1.0.0", false), Decision::Update);
        assert_eq!(decision("1.0.0-rc.2", "1.0.0", true), Decision::Update);
        // A genuinely newer dotted prerelease is NOT downgraded onto an older
        // release.
        assert_eq!(decision("1.1.0-rc.2", "1.0.0", false), Decision::UpToDate);
        assert_eq!(
            decision("1.1.0-rc.2", "1.0.0", true),
            Decision::PinnedBelowCurrent
        );
        // Two dotted prereleases of the same base stay up-to-date.
        assert_eq!(
            decision("1.0.0-rc.2", "1.0.0-rc.2", false),
            Decision::UpToDate
        );
    }

    #[test]
    fn is_prerelease_detects_only_a_dash_suffix() {
        assert!(is_prerelease("0.23.1-dev"));
        assert!(is_prerelease("1.0.0-rc.2"));
        assert!(!is_prerelease("0.23.1"));
        assert!(!is_prerelease("0.23.1+build"));
    }

    #[test]
    // spec: CLI-141
    fn check_report_reflects_the_decision_without_network() {
        // The --check branch reports pending vs up-to-date purely from the
        // decision over an explicit target version: no network is consulted.
        let pending = decision("0.2.0", "0.3.0", false);
        assert_eq!(pending, Decision::Update);
        let report = check_report("0.2.0", "0.3.0", &pending, "x86_64-unknown-linux-musl");
        assert!(report.contains("0.2.0"), "report: {report}");
        assert!(report.contains("0.3.0"), "report: {report}");
        assert!(report.contains("available"), "report: {report}");

        let current = decision("0.3.0", "0.3.0", false);
        assert_eq!(current, Decision::UpToDate);
        let report = check_report("0.3.0", "0.3.0", &current, "x86_64-unknown-linux-musl");
        assert!(report.contains("up to date"), "report: {report}");
    }

    #[test]
    // spec: CLI-147
    fn check_report_pinned_below_says_not_downgrading() {
        // The report for PinnedBelowCurrent must name both versions and say
        // "not downgrading" -- it must NOT say "up to date".
        let d = Decision::PinnedBelowCurrent;
        let report = check_report("0.3.0", "0.1.0", &d, "x86_64-unknown-linux-musl");
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
        let report = check_report("0.3.0", "0.3.0", &d, "x86_64-unknown-linux-musl");
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

    // ---- STO-65: target triple visible before download ------------------------

    #[test]
    // spec: STO-65
    fn check_report_includes_the_target_triple_in_every_branch() {
        // The resolved artifact target triple must be visible in the --check
        // report BEFORE anything is downloaded, in all three decision branches,
        // without disturbing the existing wording (each existing assertion in
        // check_report_reflects_the_decision_without_network /
        // check_report_pinned_below_says_not_downgrading /
        // check_report_up_to_date_when_equal must still hold).
        let triple = "x86_64-unknown-linux-musl";

        let update = decision("0.2.0", "0.3.0", false);
        let report = check_report("0.2.0", "0.3.0", &update, triple);
        assert!(
            report.contains(triple),
            "Update report must include the target triple: {report}"
        );
        assert!(
            report.contains("available"),
            "Update report must keep the existing wording: {report}"
        );

        let up_to_date = decision("0.3.0", "0.3.0", false);
        let report = check_report("0.3.0", "0.3.0", &up_to_date, triple);
        assert!(
            report.contains(triple),
            "UpToDate report must include the target triple: {report}"
        );
        assert!(
            report.contains("up to date"),
            "UpToDate report must keep the existing wording: {report}"
        );

        let pinned_below = Decision::PinnedBelowCurrent;
        let report = check_report("0.3.0", "0.1.0", &pinned_below, triple);
        assert!(
            report.contains(triple),
            "PinnedBelowCurrent report must include the target triple: {report}"
        );
        assert!(
            report.contains("not downgrading"),
            "PinnedBelowCurrent report must keep the existing wording: {report}"
        );
    }

    #[test]
    // spec: STO-65
    fn check_report_target_triple_distinguishes_musl_from_gnu() {
        // The whole point: a gnu -> musl artifact swap must be visible in the
        // report text, not just silently resolved. A musl triple must not read
        // as a gnu triple and vice versa.
        let musl_report = check_report(
            "0.2.0",
            "0.3.0",
            &Decision::Update,
            "x86_64-unknown-linux-musl",
        );
        assert!(musl_report.contains("musl"), "{musl_report}");
        assert!(!musl_report.contains("gnu"), "{musl_report}");

        let gnu_report = check_report(
            "0.2.0",
            "0.3.0",
            &Decision::Update,
            "x86_64-unknown-linux-gnu",
        );
        assert!(gnu_report.contains("gnu"), "{gnu_report}");
    }

    // The --json shape (the `target_triple` key added alongside the existing
    // `action`/`target`/`outcome` keys, none renamed) is covered end-to-end at
    // the CLI level in tests/cli.rs (`evolve_check_json_includes_target_triple_key`),
    // which drives the real binary and the real `print_evolve_json`, rather than
    // here: `print_evolve_json` writes straight to stdout via
    // `crate::render::print_json`, so a src/ unit test cannot observe its output
    // without capturing process stdout.

    // ---- STO-66: soft build-provenance verification ---------------------------

    #[test]
    // spec: STO-66
    fn gh_attestation_verify_args_builds_expected_argv() {
        let args = gh_attestation_verify_args("/tmp/mind-0.3.0-x86_64.tar.gz", "jaemk/mind");
        assert_eq!(
            args,
            vec![
                "attestation".to_string(),
                "verify".to_string(),
                "/tmp/mind-0.3.0-x86_64.tar.gz".to_string(),
                "--repo".to_string(),
                "jaemk/mind".to_string(),
            ],
            "argv must match `gh attestation verify <archive> --repo <repo>`: {args:?}"
        );
    }

    #[test]
    // spec: STO-66
    fn is_gh_tooling_error_classifies_known_tooling_markers() {
        assert!(
            is_gh_tooling_error("Error: unknown command \"attestation\" for \"gh\""),
            "an unsupported subcommand (old gh) must classify as a tooling error"
        );
        assert!(
            is_gh_tooling_error(
                "Get \"https://api.github.com/...\": dial tcp: lookup api.github.com: no such host"
            ),
            "a DNS/network failure must classify as a tooling error"
        );
        assert!(
            is_gh_tooling_error("Error: connection refused"),
            "a connection-refused failure must classify as a tooling error"
        );
        assert!(
            is_gh_tooling_error(
                "To use GitHub CLI in a GitHub Actions workflow, run: gh auth login"
            ),
            "an auth-required message must classify as a tooling error"
        );
        // Case-insensitive.
        assert!(is_gh_tooling_error("DIAL TCP: CONNECTION REFUSED"));
    }

    #[test]
    // spec: STO-66
    fn is_gh_tooling_error_does_not_classify_no_attestations_found_as_tooling() {
        // The critical negative case: "no attestations found" (and the raw HTTP
        // 404 wording gh also surfaces for the identical underlying condition)
        // must NOT be treated as a tooling error, because it is exactly the
        // signal a substituted artifact would also produce (the attacker cannot
        // forge a valid signed attestation for a different digest). Treating it
        // as a pass-through tooling error would defeat the entire check.
        assert!(
            !is_gh_tooling_error("Error: no attestations found"),
            "'no attestations found' must be a genuine failure, not a tooling error"
        );
        assert!(
            !is_gh_tooling_error(
                "Error: HTTP 404: Not Found (https://api.github.com/repos/jaemk/mind/attestations/sha256:deadbeef)"
            ),
            "an HTTP 404 from the attestations endpoint must be a genuine failure, not a tooling error"
        );
    }

    /// Write an executable fake `gh` at `dir/gh` that records its argv (one arg
    /// per line, skipping the leading `attestation verify <path>` positional
    /// args' exact values are still captured) to `capture_path`, then exits with
    /// `exit_code` after printing `stderr_msg` to stderr. Mirrors
    /// `write_fake_failing_curl`.
    fn write_fake_gh(dir: &Path, capture_path: &Path, exit_code: i32, stderr_msg: &str) {
        let script_path = dir.join("gh");
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {:?}\nprintf '%s' {:?} >&2\nexit {exit_code}\n",
            capture_path, stderr_msg
        );
        std::fs::write(&script_path, script).expect("write fake gh");
        let mut perms = std::fs::metadata(&script_path)
            .expect("stat fake gh")
            .permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script_path, perms).expect("chmod fake gh");
    }

    #[test]
    // spec: STO-66
    fn attestation_step_returns_none_when_gh_is_absent() {
        // A command name that resolves to nothing simulates "gh absent"
        // deterministically, without needing to hide the real `gh` (which is
        // present on this machine's PATH) from the process. No PATH mutation,
        // no ENV_LOCK needed.
        let archive = std::env::temp_dir().join("mind-evolve-attest-none-does-not-exist.tar.gz");
        assert_eq!(
            attestation_step("mind-no-such-gh-xyzzy", &archive),
            None,
            "attestation_step must return None (skip silently) when gh is absent"
        );
    }

    #[test]
    // spec: STO-66
    fn attestation_step_verified_with_fake_gh_success() {
        // A fake `gh` that exits 0 must yield Verified, and must have been
        // invoked with the expected `attestation verify <archive> --repo
        // jaemk/mind` argv (proving the wiring end-to-end, not just the pure
        // arg builder in isolation).
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let scratch =
            std::env::temp_dir().join(format!("mind-evolve-fake-gh-ok-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let capture_path = scratch.join("argv-capture.txt");
        write_fake_gh(&scratch, &capture_path, 0, "");

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{orig_path}", scratch.display());
        // SAFETY: ENV_LOCK is held for the duration of the mutation below.
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let archive = scratch.join("mind-0.3.0-x86_64.tar.gz");
        std::fs::write(&archive, b"fake archive").expect("seed fake archive");
        let result = attestation_step("gh", &archive);

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            std::env::set_var("PATH", &orig_path);
        }
        drop(guard);

        assert_eq!(
            result,
            Some(AttestationOutcome::Verified),
            "an exit-0 gh must yield Verified"
        );

        let argv = std::fs::read_to_string(&capture_path).expect("read captured argv");
        let lines: Vec<&str> = argv.lines().collect();
        assert_eq!(
            lines,
            vec![
                "attestation",
                "verify",
                archive.to_string_lossy().as_ref(),
                "--repo",
                "jaemk/mind",
            ],
            "gh must be invoked with the expected argv: {lines:?}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    // spec: STO-66
    fn attestation_step_tooling_error_with_fake_gh_network_failure() {
        // A fake `gh` that exits non-zero with a recognizably network-level
        // stderr must classify as ToolingError, so evolve proceeds with a note
        // rather than aborting -- gh never got to ask the question.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "mind-evolve-fake-gh-tooling-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let capture_path = scratch.join("argv-capture.txt");
        write_fake_gh(
            &scratch,
            &capture_path,
            1,
            "dial tcp: lookup api.github.com: no such host",
        );

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{orig_path}", scratch.display());
        // SAFETY: ENV_LOCK is held for the duration of the mutation below.
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let archive = scratch.join("mind-0.3.0-x86_64.tar.gz");
        std::fs::write(&archive, b"fake archive").expect("seed fake archive");
        let result = attestation_step("gh", &archive);

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            std::env::set_var("PATH", &orig_path);
        }
        drop(guard);

        match result {
            Some(AttestationOutcome::ToolingError(reason)) => {
                assert!(
                    reason.contains("no such host"),
                    "reason must carry gh's stderr: {reason}"
                );
            }
            other => panic!("expected Some(ToolingError(_)), got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    // spec: STO-66
    fn attestation_step_genuine_failure_with_fake_gh_no_attestations_aborts() {
        // A fake `gh` that exits non-zero reporting "no attestations found"
        // must classify as GenuineFailure -- the exact case that must abort the
        // swap rather than pass through, since it is indistinguishable (from
        // gh's output) from a substituted artifact.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let n = MKTEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "mind-evolve-fake-gh-fail-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let capture_path = scratch.join("argv-capture.txt");
        write_fake_gh(&scratch, &capture_path, 1, "Error: no attestations found");

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{orig_path}", scratch.display());
        // SAFETY: ENV_LOCK is held for the duration of the mutation below.
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let archive = scratch.join("mind-0.3.0-x86_64.tar.gz");
        std::fs::write(&archive, b"fake archive").expect("seed fake archive");
        let result = attestation_step("gh", &archive);

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            std::env::set_var("PATH", &orig_path);
        }
        drop(guard);

        match result {
            Some(AttestationOutcome::GenuineFailure(reason)) => {
                assert!(
                    reason.contains("no attestations found"),
                    "reason must carry gh's stderr: {reason}"
                );
                // Mirror what download_and_swap does with this outcome: it must
                // map to AttestationVerificationFailed, which aborts (returns
                // Err) rather than proceeding to extraction/swap.
                let mapped = MindError::AttestationVerificationFailed { reason };
                assert_eq!(mapped.kind(), "attestation-verification-failed");
            }
            other => panic!("expected Some(GenuineFailure(_)), got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
