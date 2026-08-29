//! `mind evolve` -- update the `mind` binary itself in place.
//!
//! The download, verification, extraction, and in-place swap are the
//! `self_update` crate's, driven through its github backend so the artifact
//! selection matches what `resources/install.sh` and the Homebrew formula
//! resolve (`mind-<version>-<target>.tar.gz`). What lives here is everything
//! around that: the platform triple, the up-to-date/downgrade decision, the
//! managed-policy check, the confirmation prompt, the `--json` result, and the
//! build-provenance gate wired in as the crate's pre-extraction archive hook.
//!
//! The decision logic (target triple, version comparison, report text) is pure,
//! so it is unit-testable with no network access. Only `run`'s tail, and the
//! updater it builds, touch the network.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use self_update::backends::github;

use crate::error::{MindError, Result};
use crate::mindfile::version_at_least;

const REPO_OWNER: &str = "jaemk";
const REPO_NAME: &str = "mind";
const REPO: &str = "jaemk/mind";

/// The release asset carrying the published digests (STO-47).
const SHA256SUMS_ASSET: &str = "SHA256SUMS";

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
/// returned regardless of `explicit`. When `current` and `target` share the
/// same NUMERIC base version (a "tie") but differ in their prerelease suffix
/// (or exactly one of them carries one), the tie is broken by prerelease
/// precedence -- see [`tie_break`] -- rather than assumed `UpToDate` (STO-77).
// spec: CLI-140 STO-77
pub fn decision(current: &str, target: &str, explicit: bool) -> Decision {
    if version_at_least(current, target) {
        if version_at_least(target, current) {
            // Both directions hold: current and target share the same
            // numeric base version. A byte-identical match aside, this is
            // NOT necessarily "up to date" -- a prerelease predates its base
            // release, and two prereleases of the same base order against
            // each other by semver precedence, so hand off to `tie_break`
            // rather than assuming `UpToDate` outright.
            return tie_break(current, target, explicit);
        }
        // current is STRICTLY above target numerically.
        if explicit {
            // target < current: explicit downgrade request we refuse.
            Decision::PinnedBelowCurrent
        } else {
            Decision::UpToDate
        }
    } else {
        Decision::Update
    }
}

/// Resolve a NUMERIC TIE between `current` and `target` (the same dotted base
/// version, as already established by the caller via `version_at_least` in
/// both directions) into a `Decision`, breaking the tie by prerelease
/// precedence instead of assuming `UpToDate` outright (STO-77).
///
/// Before this existed, `decision` only ever moved off a numeric tie in ONE
/// direction: a prerelease `current` onto its own (non-prerelease) base
/// `target`. Two other same-base pairings silently fell through to
/// `UpToDate`/no-op even under an explicit `--to`: two prereleases of the
/// same base ordered against each other (e.g. running `0.24.0-rc1`, pinning
/// `--to 0.24.0-rc2` -- the main reason to pin an rc at all), and a release
/// `current` explicitly pinned onto a same-base prerelease `target` (e.g.
/// running the released `0.24.0`, `--to 0.24.0-rc1`). Both are handled here.
///
/// - Byte-identical `current`/`target` strings are always `UpToDate`.
/// - A prerelease `current` vs. a plain-release `target` of the same base:
///   the prerelease predates the release, so `Update` (unconditional on
///   `explicit`, matching the pre-existing behavior this generalizes).
/// - A plain-release `current` vs. a same-base prerelease `target`: the
///   prerelease predates `current`, so an EXPLICIT pin here is a downgrade
///   request (`PinnedBelowCurrent`); a non-explicit target never reaches this
///   arm in practice (`releases/latest` never returns a prerelease), so it
///   degrades to `UpToDate` rather than a spurious refusal if it ever did.
/// - Two prereleases of the same base: compared via [`prerelease_cmp`]
///   (semver precedence). `target` ordering ABOVE `current` is `Update`;
///   BELOW is an explicit downgrade (`PinnedBelowCurrent`) or a non-explicit
///   `UpToDate`; equal precedence (but non-identical strings, e.g. differing
///   only in build metadata) is `UpToDate`.
fn tie_break(current: &str, target: &str, explicit: bool) -> Decision {
    if current == target {
        return Decision::UpToDate;
    }
    match (is_prerelease(current), is_prerelease(target)) {
        (false, false) => Decision::UpToDate,
        (true, false) => Decision::Update,
        (false, true) => {
            if explicit {
                Decision::PinnedBelowCurrent
            } else {
                Decision::UpToDate
            }
        }
        (true, true) => {
            let cp = prerelease_suffix(current).unwrap_or_default();
            let tp = prerelease_suffix(target).unwrap_or_default();
            match prerelease_cmp(cp, tp) {
                std::cmp::Ordering::Less => Decision::Update,
                std::cmp::Ordering::Equal => Decision::UpToDate,
                std::cmp::Ordering::Greater => {
                    if explicit {
                        Decision::PinnedBelowCurrent
                    } else {
                        Decision::UpToDate
                    }
                }
            }
        }
    }
}

/// Whether `v` carries a prerelease suffix (a `-` segment, e.g. `-dev` or
/// `-rc.2`, such as a fork or packager might append). Build metadata (`+...`)
/// is not a prerelease. Used by [`tie_break`] to classify a numeric-tie pair
/// before ordering it.
fn is_prerelease(v: &str) -> bool {
    v.split_once('+').map_or(v, |(base, _)| base).contains('-')
}

/// The prerelease suffix of `v` (the text after its first `-`, before any
/// `+build` segment), if any. `None` when `v` carries no prerelease.
///
/// Whole-string build-metadata stripping happens first (mirroring
/// `version_at_least`'s rationale): a `+` inside a dotted build-metadata
/// segment must never be mistaken for part of the prerelease suffix.
fn prerelease_suffix(v: &str) -> Option<&str> {
    let base_and_pre = v.split_once('+').map_or(v, |(base, _)| base);
    base_and_pre.split_once('-').map(|(_, pre)| pre)
}

/// Compare two semver-shaped dot-identifiers by semver precedence (informally
/// mirroring semver.org's precedence rule #11): a purely-numeric identifier
/// (all ASCII digits) compares numerically; anything else compares as ASCII
/// bytes; a numeric identifier ALWAYS has lower precedence than a
/// non-numeric one when the two are compared against each other (so `"9"` <
/// `"rc"`, not just `"9"` < `"9a"`).
fn semver_identifier_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a_numeric = !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit());
    let b_numeric = !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit());
    match (a_numeric, b_numeric) {
        (true, true) => {
            // Values reaching here are already validated by
            // `is_plausible_release_tag`, but parse defensively (u128, with a
            // MAX fallback) rather than panicking on an implausibly long
            // numeric identifier.
            let an: u128 = a.parse().unwrap_or(u128::MAX);
            let bn: u128 = b.parse().unwrap_or(u128::MAX);
            an.cmp(&bn)
        }
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

/// Compare two prerelease suffixes (the text after `-`, before any `+build`)
/// by semver precedence: dot-separated identifiers compared pairwise via
/// [`semver_identifier_cmp`]; when one is a strict prefix of the other in
/// dot-identifiers (fewer fields), the SHORTER one has lower precedence
/// (`rc.1` < `rc.1.2`), matching semver.org's precedence rule #11.
fn prerelease_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let c = semver_identifier_cmp(x, y);
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
        }
    }
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

    // Resolve the target version: an explicit --to (or a policy pin) bypasses the
    // network entirely; otherwise the latest published release is read through an
    // updater built for the lookup alone. Either way the resolved value is
    // validated before anything uses it (STO-76, STO-77).
    let explicit = version.is_some();
    let target_version = match version.as_deref() {
        Some(v) => validated_target(v)?,
        None => validated_target(&latest_version(&updater(target, None, lookup_timeout())?)?)?,
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

    // Pin the updater to the version that was decided on and confirmed, so the
    // release that gets installed is that one even if a newer one is published in
    // between.
    let upd = updater(target, Some(&target_version), download_timeout())?;
    download_and_swap(upd, current, &target_version, target)
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

/// The github updater for the `mind` release repo.
///
/// Everything the crate would drive itself is turned off: `evolve` prints its own
/// progress lines and runs its own `[y/N]` prompt (CLI-141) after its own
/// decision (CLI-140, STO-77), so the crate's release-status block and
/// confirmation are suppressed. What it *is* used for is the transport, the
/// asset selection, the verification gates, and the swap.
///
/// `tag` pins a release (`--to`, or a managed-policy pin); `None` leaves the
/// updater on the latest release, which is what `latest_version` reads.
///
/// - `target` is the resolved triple (STO-65), so the musl artifact is selected
///   on linux exactly as `resources/install.sh` does.
/// - `checksum_from_asset("SHA256SUMS")` is STO-47: the digests published beside
///   the release are fetched and the entry for the selected artifact is verified
///   before anything is extracted.
/// - `verify_archive` is STO-66: `gh attestation verify` over the downloaded
///   archive, which is the file the release workflow attests.
/// - `check_install_path_writable(true)` moves the "cannot replace this binary"
///   failure (STO-45) ahead of the download instead of after it.
/// - `timeout` bounds each request the updater makes. It is a WHOLE-request
///   budget, body included, not a connect timeout, so the two callers pass
///   different values: see `lookup_timeout` and `download_timeout` (STO-52).
fn updater(
    target: &str,
    tag: Option<&str>,
    timeout: std::time::Duration,
) -> Result<github::Update> {
    let out = crate::render::ctx();
    let mut builder = github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name("mind")
        .target(target)
        .current_version(env!("CARGO_PKG_VERSION"))
        // GH_TOKEN / GITHUB_TOKEN when set, which lifts GitHub's 60/hour
        // per-IP anonymous limit; absent, the request goes out unauthenticated.
        .auth_token_from_env()
        // spec: STO-52
        .timeout(timeout)
        .no_confirm(true)
        .show_output(false)
        // The progress bar writes to the terminal, so it is off under --json,
        // whose output has to stay machine-readable (CLI-153).
        .show_download_progress(!out.json)
        .check_install_path_writable(true)
        // spec: STO-47
        .checksum_from_asset(SHA256SUMS_ASSET)
        // spec: STO-66
        .verify_archive(|archive: &Path| attestation_gate(archive));
    if let Some(tag) = tag {
        builder.release_tag(format!("v{tag}"));
    }
    builder.build().map_err(map_update_error)
}

/// Validate a resolved target version and normalize it (one leading `v` off).
///
/// The value reaches here either from an explicit `--to` (or a managed-policy
/// pin) or from the release API's `tag_name`, and is validated the same way in
/// both cases, immediately on resolution and before anything downstream uses it:
/// the release-tag lookup it drives, the confirmation prompt it is echoed into,
/// and the `--json` result.
///
/// spec: STO-76 -- a value carrying path segments (a repo/release takeover, a
/// TLS-intercepting proxy, or a `--to` copied from a malicious "run these steps"
/// doc) would otherwise be interpolated into a request path and shown to the
/// user as the version they are approving.
///
/// This uses `is_plausible_release_tag`, NOT `is_plausible_version`: the two
/// answer different questions (see `is_plausible_release_tag`'s doc). A release
/// tag legitimately carries a semver prerelease/build suffix (`evolve --to
/// 1.2.3-rc1` is the only way to reach a prerelease, since `releases/latest`
/// never surfaces one), so the validator accepts that shape while still
/// rejecting anything that could escape or split a URL path segment.
///
/// spec: STO-77 -- a rejected value is `SelfUpdateInvalidTarget`, NOT
/// `SelfUpdatePolicy`: the latter's JSON `kind` (`self-update-policy`) reads as
/// a managed-policy refusal, which this is not.
///
/// Only ONE leading `v` is stripped, so `--to v1.2.3` behaves identically to
/// `--to 1.2.3` while `--to vv1.2.3` still fails validation.
// spec: STO-76 STO-77
fn validated_target(raw: &str) -> Result<String> {
    let value = raw.strip_prefix('v').unwrap_or(raw).to_string();
    if !crate::mindfile::is_plausible_release_tag(&value) {
        return Err(MindError::SelfUpdateInvalidTarget { value });
    }
    Ok(value)
}

/// The version of the latest published release (the `releases/latest` endpoint,
/// which never surfaces a prerelease), with the leading `v` already stripped.
fn latest_version(upd: &github::Update) -> Result<String> {
    let releases = upd.get_latest_release().map_err(map_update_error)?;
    let release = releases.latest().ok_or_else(|| MindError::DownloadFailed {
        url: format!("https://api.github.com/repos/{REPO}/releases/latest"),
        reason: "the release listing is empty".to_string(),
    })?;
    Ok(release.version().to_string())
}

/// STO-66's build-provenance gate, as the crate's pre-extraction archive hook.
///
/// The hook runs on the downloaded archive, which is what
/// `actions/attest-build-provenance` signs (the release workflow's
/// `subject-path` is `mind-*.tar.gz`), so this is the file `gh` has to be
/// pointed at. Absent `gh`, or a `gh` that could not complete the check, this
/// proceeds with at most a note; a genuine verification failure returns `Err`,
/// which aborts with nothing extracted and nothing replaced.
fn attestation_gate(archive: &Path) -> std::result::Result<(), self_update::Error> {
    let out = crate::render::ctx();
    match attestation_step("gh", archive) {
        None => Ok(()),
        Some(AttestationOutcome::Verified) => {
            if !out.json {
                println!("{} build provenance verified", out.ok());
            }
            Ok(())
        }
        Some(AttestationOutcome::ToolingError(reason)) => {
            if !out.json {
                println!(
                    "{} build provenance could not be verified ({reason}); continuing",
                    out.warn()
                );
            }
            Ok(())
        }
        Some(AttestationOutcome::GenuineFailure(reason)) => {
            Err(self_update::Error::archive_verification_rejected(reason))
        }
    }
}

/// Download the release archive, verify it, and replace the running executable.
///
/// Holds the global exclusive lock (STO-46) for the entire download-and-swap
/// step so two concurrent `mind evolve` invocations cannot race. The lock is
/// taken here, after the network-free decision and the prompt, so `evolve
/// --check` and a declined prompt never contend for it (STO-48).
fn download_and_swap(
    upd: github::Update,
    current: &str,
    target_version: &str,
    target_triple: &str,
) -> Result<()> {
    // spec: STO-46 -- hold the exclusive lock for the entire download-and-swap.
    let paths = crate::paths::Paths::resolve()?;
    let mut lock = crate::lock::open(&paths)?;
    let _guard = lock.write()?;

    let out = crate::render::ctx();
    if let Some(line) = download_banner(out.json, target_version, target_triple) {
        println!("{line}");
    }

    upd.update().map_err(map_update_error)?;

    if out.json {
        return print_evolve_json(target_version, "updated", target_triple);
    }
    println!("{} updated mind {current} -> {target_version}", out.ok());
    Ok(())
}

/// The "downloading mind <version>" line, or `None` under `--json`.
///
/// `evolve` writes its own document straight to stdout rather than through
/// `commands.rs`, so it gets none of CLI-217's structural protection: this guard
/// is the only thing keeping a progress line out of `--json` stdout, which has to
/// stay a single machine-readable document. Returning an `Option` rather than
/// printing inline keeps that decision unit-testable without a download.
// spec: CLI-217
fn download_banner(json: bool, target_version: &str, target_triple: &str) -> Option<String> {
    if json {
        return None;
    }
    let out = crate::render::ctx();
    Some(format!(
        "{} downloading mind {target_version} ({})",
        out.bullet(),
        out.dim(&format!("{target_triple} from {REPO}"))
    ))
}

/// Map an updater failure onto mind's structured errors (no stringly-typed
/// errors leak out of this module).
///
/// The mappings that matter are the ones a caller acts on: a rejected
/// attestation (STO-66) and a digest mismatch (STO-47) keep their own variants
/// and exit codes rather than collapsing into a generic download failure, and
/// an unwritable target keeps the actionable "reinstall with privileges"
/// wording (STO-45). Anything else is a download failure carrying the crate's
/// message, sanitized: a hostile endpoint controls parts of it (STO-54).
fn map_update_error(e: self_update::Error) -> MindError {
    let release_url = format!("https://github.com/{REPO}/releases");
    match e {
        // spec: STO-66
        self_update::Error::ArchiveVerificationRejected { reason, .. } => {
            MindError::AttestationVerificationFailed {
                reason: crate::sanitize::strip_ansi(&reason.unwrap_or_default()),
            }
        }
        // spec: STO-47
        self_update::Error::ChecksumMismatch {
            expected, computed, ..
        } => MindError::DigestMismatch {
            url: release_url,
            expected,
            actual: computed,
        },
        // spec: STO-47 -- a release with no usable SHA256SUMS entry is a
        // refusal, not a silently unverified install.
        self_update::Error::ChecksumSourceInvalid { asset, reason, .. } => {
            MindError::DigestMismatch {
                url: release_url,
                expected: format!(
                    "(from {SHA256SUMS_ASSET}: {})",
                    crate::sanitize::strip_ansi(&reason)
                ),
                actual: crate::sanitize::strip_ansi(&asset),
            }
        }
        // spec: STO-45
        self_update::Error::InstallPathNotWritable { path, .. } => MindError::TargetNotWritable {
            path: path.display().to_string(),
        },
        // No asset for this platform's triple in the release.
        self_update::Error::NoReleaseFound { .. } => MindError::ReleaseAssetEmpty,
        other => MindError::DownloadFailed {
            url: release_url,
            reason: maybe_proxy_hint(&other.to_string()),
        },
    }
}

/// Phrases that identify a TLS trust failure rather than a transport failure
/// (STO-79). Matched case-insensitively against the sanitized reason.
///
/// The first is rustls's own wording, which is what mind's client produces --
/// verified against a local server presenting a cert from an untrusted CA. The
/// rest cover the other spellings a reader is likely to paste into a bug
/// report (OpenSSL's, and the self-signed variant of the same root cause).
const CERT_MARKERS: &[&str] = &[
    "invalid peer certificate",
    "certificate verify",
    "unknownissuer",
    "self-signed certificate",
];

/// Append a setup hint when the failure has a known, actionable cause.
///
/// Two causes, with different fixes, so they get different hints and never both
/// fire: a proxy error means the request could not REACH the endpoint, a
/// certificate error means it reached it and would not TRUST it. Proxy is
/// checked first because a 407 is answered by the proxy before any certificate
/// is presented.
///
/// The text comes from the HTTP client and, through it, from the endpoint,
/// which is untrusted (a MITM'd or hostile endpoint controls those bytes). It
/// is sanitized via `strip_ansi` before being embedded in the returned string
/// (STO-54).
// spec: STO-54, STO-79
fn maybe_proxy_hint(reason: &str) -> String {
    let reason = crate::sanitize::strip_ansi(reason);
    let lower = reason.to_ascii_lowercase();
    if reason.contains("407") || lower.contains("proxy") {
        format!(
            "{reason}\nhint: if you are behind a proxy, set HTTPS_PROXY or HTTP_PROXY \
             (e.g. export HTTPS_PROXY=http://proxy.example.com:8080)"
        )
    } else if CERT_MARKERS.iter().any(|m| lower.contains(m)) {
        // mind verifies against the machine's trust store (STO-78), so the fix
        // is to put the CA there -- not to relax verification, which `evolve`
        // offers no way to do.
        format!(
            "{reason}\nhint: mind verifies TLS against your machine's certificate store. \
             If your network intercepts HTTPS with a company CA, install that CA in the \
             system store, or point mind at a bundle containing it \
             (e.g. export SSL_CERT_FILE=/path/to/corporate-ca.pem)"
        )
    } else {
        reason
    }
}

/// Per-process counter that makes successive `mktemp_dir` calls within the same
/// process yield distinct paths even when the wall-clock resolution is coarser
/// than the interval between calls.
static MKTEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create an unpredictably-named, exclusively-owned temp directory under a
/// caller-chosen name prefix. The name combines the prefix, the PID, a subsecond
/// wall-clock timestamp, and a per-process sequence number so that:
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
/// The only caller left is `tui::action`'s stdout-capture file (TUI-61); the
/// `evolve` download staging that used to share this is the updater's own
/// `tempfile::TempDir` now.
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

/// The ceiling on the artifact download (STO-52), matching the `--max-time 600`
/// `resources/install.sh` passes to curl.
const DOWNLOAD_TIMEOUT_CEILING_SECS: u64 = 600;

/// The budget for a metadata request (the release lookup, and the `SHA256SUMS`
/// fetch that rides along with the download updater): `MIND_HTTP_TIMEOUT_SECS`,
/// default 15 s.
fn lookup_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(http_timeout_secs())
}

/// The budget for the download (STO-52).
///
/// The updater's timeout is a WHOLE-request budget, body included, not curl's
/// connect timeout, so the 15 s that bounds a metadata request would abort a
/// perfectly healthy multi-megabyte download on a slow link. The download gets
/// the 600 s ceiling instead, and a `MIND_HTTP_TIMEOUT_SECS` set ABOVE that wins
/// (someone raising the knob wants more room, not less).
fn download_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(http_timeout_secs().max(DOWNLOAD_TIMEOUT_CEILING_SECS))
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

    #[test]
    // spec: STO-76 STO-77
    fn run_refuses_an_explicit_to_target_with_path_segments_with_no_network_call() {
        // An explicit `--to` value bypasses the network entirely for target
        // resolution (no `latest` lookup), so a malicious value must still be
        // refused purely from local validation -- no PATH stub needed; if
        // `run` somehow tried to shell out despite there being no curl/wget
        // stub on PATH, it would fail with a `DownloadFailed`/`Io` error
        // instead of `SelfUpdateInvalidTarget`, and the assertion below would
        // catch that misbehavior.
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
            // spec: STO-77
            Err(MindError::SelfUpdateInvalidTarget { value }) => {
                assert!(
                    value.contains("attacker"),
                    "must name the rejected --to value: {value}"
                );
            }
            other => panic!("expected a SelfUpdateInvalidTarget refusal, got {other:?}"),
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
        // and (since the target is built numerically ABOVE the running
        // version) come back `Decision::Update`. No network call is possible
        // here at all (no curl/wget stub is put on PATH), so reaching the
        // `!yes` confirmation gate -- rather than a `DownloadFailed`/`Io`
        // error from an attempted shell-out, or a `SelfUpdateInvalidTarget`
        // "not a plausible" refusal -- proves both that validation accepted
        // the value AND that `decision()` classified it as an update,
        // end-to-end through production code.
        //
        // L8: the target is DERIVED from `CARGO_PKG_VERSION` (bumping the
        // major component) rather than hardcoded as `"1.2.3-rc1"`. A
        // hardcoded literal only worked because it happened to sit
        // numerically above this crate's version at the time it was written
        // (0.23.0); once the crate's own version reaches 1.2.3, the same
        // literal ties instead of exceeding it, `decision()` flips outcome,
        // and this test would fail with an unrelated-looking message far
        // from its real cause.
        let current = env!("CARGO_PKG_VERSION");
        let major: u64 = current
            .split(['.', '-', '+'])
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let target = format!("{}.0.0-rc1", major + 1);

        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        let orig_tty = std::env::var("MIND_TTY").ok();
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::set_var("MIND_TTY", "0");
        }

        let result = run(false, false, Some(target.clone()));

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
                    action.contains(&target),
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
    // spec: STO-76 STO-77
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
            // spec: STO-77
            Err(MindError::SelfUpdateInvalidTarget { value }) => {
                assert!(
                    value.contains("1.0.0-../.."),
                    "must name the rejected --to value: {value}"
                );
            }
            other => panic!("expected a SelfUpdateInvalidTarget refusal, got {other:?}"),
        }
    }

    #[test]
    // spec: STO-76
    fn run_refuses_a_pure_dot_run_smuggled_inside_an_explicit_prerelease_suffix() {
        // L9: `run_refuses_traversal_smuggled_inside_an_explicit_prerelease_suffix`
        // (above) uses a `/`-carrying payload, which `is_plausible_version`
        // (the OLD, pre-STO-76 validator) also rejects outright -- ANY dash
        // fails that validator, since it is digits-and-dots only. So that
        // test would stay green even if the `run()` call site regressed back
        // to calling `is_plausible_version` instead of
        // `is_plausible_release_tag`, which would ALSO wrongly break every
        // legitimate `--to X-rc1` pin (covered separately by
        // `run_accepts_an_explicit_prerelease_to_target_and_reaches_decision_end_to_end`).
        //
        // This test instead pins the SPLIT ITSELF: a prerelease suffix built
        // from nothing but dots (no slash at all) is exactly the shape a
        // naive "every character is in the allowed charset" reading of
        // `is_plausible_release_tag`'s grammar would wrongly accept (`.` is
        // itself a member of the allowed suffix charset), but the real
        // implementation splits the suffix on `.` and rejects any EMPTY
        // identifier between two dots -- so it is still refused. A validator
        // that dropped that non-empty-identifier check (while still using
        // the right function at the call site) would pass this test's
        // predecessor but fail this one.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig_policy_file = std::env::var("MIND_POLICY_FILE").ok();
        // SAFETY: ENV_LOCK is held for the duration of the mutation and the
        // `run()` call below.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
        }

        let result = run(true, false, Some("1.0.0-..".to_string()));

        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match orig_policy_file {
                Some(v) => std::env::set_var("MIND_POLICY_FILE", v),
                None => std::env::remove_var("MIND_POLICY_FILE"),
            }
        }
        drop(guard);

        match result {
            Err(MindError::SelfUpdateInvalidTarget { value }) => {
                assert!(
                    value.contains("1.0.0-.."),
                    "must name the rejected --to value: {value}"
                );
            }
            other => panic!("expected a SelfUpdateInvalidTarget refusal, got {other:?}"),
        }
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
    fn maybe_proxy_hint_names_only_settings_that_apply() {
        // The hint must name the environment variables the updater's HTTP client
        // actually reads, and must not point at settings that do nothing for it:
        // git's `http.proxy` never applied, and `~/.curlrc` stopped applying when
        // the downloader stopped being curl.
        let hint = maybe_proxy_hint("Received HTTP code 407 from proxy");
        assert!(
            !hint.contains("http.proxy"),
            "hint must not mention git's http.proxy: {hint:?}"
        );
        assert!(
            !hint.contains("curlrc"),
            "hint must not mention ~/.curlrc, which the updater does not read: {hint:?}"
        );
        assert!(
            hint.contains("HTTPS_PROXY") && hint.contains("HTTP_PROXY"),
            "hint must name HTTPS_PROXY and HTTP_PROXY: {hint:?}"
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

    #[test]
    // spec: STO-78
    fn the_self_update_dependency_trusts_the_machines_certificate_store() {
        // STO-78 is a Cargo feature, not a code path, so this is where it can be
        // pinned: drop `native-certs` and every build still compiles and every
        // other test still passes, while `evolve` silently stops working on any
        // network that re-signs HTTPS with a company CA. The failure is invisible
        // to a hermetic suite and only shows up on a user's machine, which is
        // exactly the kind of regression worth a guard.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let manifest = std::fs::read_to_string(&path).expect("Cargo.toml must be readable");
        let dep = manifest
            .split_once("self_update = {")
            .expect("Cargo.toml must declare the self_update dependency")
            .1
            .split_once('}')
            .expect("the self_update dependency table must be closed")
            .0;
        assert!(
            dep.contains("\"native-certs\""),
            "self_update must enable `native-certs` so evolve verifies against the \
             machine's certificate store (STO-78); got: {dep}"
        );
    }

    #[test]
    // spec: STO-79
    fn maybe_proxy_hint_certificate_failure_points_at_the_trust_store() {
        // The exact string rustls produces when the peer presents a cert from a CA
        // that is not in the trust store, captured from a live handshake against a
        // server holding a cert signed by an untrusted CA. This is what a user
        // behind an intercepting corporate proxy sees, so it must carry the hint
        // that names the fix (the trust store / SSL_CERT_FILE) rather than the
        // proxy hint, which would send them after the wrong setting.
        let reason = "io: invalid peer certificate: UnknownIssuer";
        let result = maybe_proxy_hint(reason);
        assert!(
            result.starts_with(reason),
            "the original reason must be preserved ahead of the hint: {result:?}"
        );
        assert!(
            result.contains("SSL_CERT_FILE"),
            "a certificate failure must name the bundle override: {result:?}"
        );
        assert!(
            result.contains("certificate store"),
            "a certificate failure must name the machine's trust store: {result:?}"
        );
        assert!(
            !result.contains("HTTPS_PROXY"),
            "a certificate failure must NOT get the proxy hint: {result:?}"
        );
    }

    #[test]
    // spec: STO-79
    fn maybe_proxy_hint_certificate_markers_are_matched_case_insensitively() {
        // The reason text is whatever the transport produced, so the marker match
        // must not depend on its casing. Each recognized spelling gets the cert
        // hint, in the casing least likely to match a naive `contains`.
        for reason in [
            "Invalid Peer Certificate: UnknownIssuer",
            "SSL certificate problem: unable to get local issuer certificate verify failed",
            "tls handshake failed: UNKNOWNISSUER",
            "Self-Signed Certificate in certificate chain",
        ] {
            let result = maybe_proxy_hint(reason);
            assert!(
                result.contains("SSL_CERT_FILE"),
                "{reason:?} must be recognized as a certificate failure: {result:?}"
            );
        }
    }

    #[test]
    // spec: STO-54, STO-79
    fn maybe_proxy_hint_prefers_the_proxy_hint_when_both_could_match() {
        // A 407 is answered by the proxy before any certificate is presented, so a
        // reason carrying both signals is a proxy problem. The two hints are
        // mutually exclusive; this pins which one wins.
        let reason = "407 Proxy Authentication Required (invalid peer certificate: UnknownIssuer)";
        let result = maybe_proxy_hint(reason);
        assert!(
            result.contains("HTTPS_PROXY"),
            "a 407 must take the proxy hint even alongside certificate wording: {result:?}"
        );
        assert!(
            !result.contains("SSL_CERT_FILE"),
            "the two hints must be mutually exclusive: {result:?}"
        );
    }

    #[test]
    // spec: STO-79
    fn maybe_proxy_hint_strips_ansi_and_bidi_from_a_certificate_reason() {
        // The certificate branch is reached with endpoint-controlled text just like
        // the proxy branch (STO-54), so it must sanitize before it embeds. A hostile
        // subject/issuer name is exactly where spoofed control sequences would ride in.
        let hostile = "\x1b[1minvalid peer certificate\x1b[0m: \u{202E}UnknownIssuer";
        let result = maybe_proxy_hint(hostile);
        assert!(
            !result.contains('\x1b'),
            "ANSI must be stripped in the certificate branch: {result:?}"
        );
        assert!(
            !result.contains('\u{202E}'),
            "bidi overrides must be stripped in the certificate branch: {result:?}"
        );
        assert!(
            result.contains("SSL_CERT_FILE"),
            "sanitization must not suppress the certificate hint: {result:?}"
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

    // ---- STO-77 / M8: numeric-tie prerelease ordering -------------------

    #[test]
    // spec: STO-77 CLI-140
    fn decision_explicit_prerelease_to_prerelease_tie_break_orders_by_precedence() {
        // M8: before this fix, `evolve --to 0.24.0-rc2` while running
        // `0.24.0-rc1` tied numerically, and since `target` ALSO carried a
        // prerelease suffix, the old `is_prerelease(current) &&
        // !is_prerelease(target)` guard never fired -- the whole point of
        // pinning an rc release. `decision` silently returned `UpToDate`
        // ("already up to date", exit 0, nothing changed) instead of
        // offering the update. This is the headline case the fix restores:
        // moving forward between two prereleases of the same base must
        // offer `Update`, and moving backward must be refused as an explicit
        // downgrade, not silently ignored.
        assert_eq!(
            decision("0.24.0-rc1", "0.24.0-rc2", true),
            Decision::Update,
            "pinning a HIGHER rc of the same base must offer the update"
        );
        assert_eq!(
            decision("0.24.0-rc2", "0.24.0-rc1", true),
            Decision::PinnedBelowCurrent,
            "pinning a LOWER rc of the same base must be a refused downgrade, not UpToDate"
        );
        // The non-explicit path (a fetched `latest` tag, which never carries
        // a prerelease in practice) still degrades to `UpToDate` rather than
        // ever reporting a spurious downgrade refusal.
        assert_eq!(
            decision("0.24.0-rc2", "0.24.0-rc1", false),
            Decision::UpToDate
        );
        // Byte-identical prerelease strings are always up to date.
        assert_eq!(
            decision("0.24.0-rc1", "0.24.0-rc1", true),
            Decision::UpToDate
        );

        // Numeric-identifier precedence is compared NUMERICALLY, not
        // lexicographically: "9" must order below "10" (a naive string
        // compare would read "10" < "9").
        assert_eq!(decision("0.24.0-9", "0.24.0-10", true), Decision::Update);
        assert_eq!(
            decision("0.24.0-10", "0.24.0-9", true),
            Decision::PinnedBelowCurrent
        );

        // A shorter dot-identifier list has LOWER precedence than a longer
        // one that extends it (semver.org precedence rule #11: `rc.1` <
        // `rc.1.2`).
        assert_eq!(
            decision("0.24.0-rc.1", "0.24.0-rc.1.2", true),
            Decision::Update
        );
        assert_eq!(
            decision("0.24.0-rc.1.2", "0.24.0-rc.1", true),
            Decision::PinnedBelowCurrent
        );
    }

    #[test]
    // spec: STO-77 CLI-140
    fn decision_explicit_release_to_same_base_prerelease_is_pinned_below_current() {
        // M8's second silent no-op: running the RELEASED `0.24.0` and pinning
        // `--to 0.24.0-rc1` also ties numerically. A prerelease predates its
        // base release (the same rule the pre-existing "prerelease current ->
        // release target" arm already encoded in the other direction), so an
        // EXPLICIT pin back onto a same-base prerelease is a downgrade
        // request and must be refused as such, not silently reported
        // "already up to date".
        assert_eq!(
            decision("0.24.0", "0.24.0-rc1", true),
            Decision::PinnedBelowCurrent
        );
        // Without an explicit pin (unreachable in practice -- `releases/latest`
        // never returns a prerelease -- but must degrade safely rather than
        // spuriously refuse) it stays UpToDate.
        assert_eq!(decision("0.24.0", "0.24.0-rc1", false), Decision::UpToDate);
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

    // spec: STO-52
    #[test]
    fn the_download_budget_is_never_the_metadata_budget() {
        // The updater's timeout is a whole-request budget, body included, not a
        // connect timeout, so the value that bounds a release lookup would abort a
        // healthy multi-megabyte download on a slow link. The download gets the
        // ceiling instead, and a knob set above the ceiling still wins.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig = std::env::var("MIND_HTTP_TIMEOUT_SECS").ok();

        // SAFETY: ENV_LOCK is held across every mutation and read below.
        unsafe { std::env::remove_var("MIND_HTTP_TIMEOUT_SECS") };
        let (default_lookup, default_download) = (lookup_timeout(), download_timeout());

        unsafe { std::env::set_var("MIND_HTTP_TIMEOUT_SECS", "30") };
        let (raised_lookup, raised_download) = (lookup_timeout(), download_timeout());

        unsafe { std::env::set_var("MIND_HTTP_TIMEOUT_SECS", "900") };
        let above_ceiling = download_timeout();

        // Restore before any assertion can panic and leave the env corrupted.
        // SAFETY: ENV_LOCK is still held.
        unsafe {
            match orig {
                Some(v) => std::env::set_var("MIND_HTTP_TIMEOUT_SECS", v),
                None => std::env::remove_var("MIND_HTTP_TIMEOUT_SECS"),
            }
        }
        drop(guard);

        assert_eq!(default_lookup.as_secs(), 15, "the default metadata budget");
        assert_eq!(
            default_download.as_secs(),
            600,
            "the download must get the 600s ceiling, not the 15s metadata budget"
        );
        assert_eq!(
            raised_lookup.as_secs(),
            30,
            "MIND_HTTP_TIMEOUT_SECS sets the metadata budget"
        );
        assert_eq!(
            raised_download.as_secs(),
            600,
            "a knob below the ceiling must not shrink the download budget"
        );
        assert_eq!(
            above_ceiling.as_secs(),
            900,
            "a knob above the ceiling wins: raising it asks for more room, not less"
        );
    }

    // spec: STO-76
    #[test]
    fn validated_target_strips_exactly_one_leading_v() {
        assert_eq!(validated_target("1.2.3").unwrap(), "1.2.3");
        assert_eq!(
            validated_target("v1.2.3").unwrap(),
            "1.2.3",
            "`--to v1.2.3` must behave identically to `--to 1.2.3`"
        );
        assert_eq!(validated_target("v1.2.3-rc1").unwrap(), "1.2.3-rc1");
        assert!(
            validated_target("vv1.2.3").is_err(),
            "only one leading `v` is stripped, so the second one must fail validation"
        );
    }

    // The release tag the updater looks up is `v<version>`, built from the
    // ALREADY-validated (and `v`-stripped) version. Pinning `--to v1.2.3` must
    // therefore request `v1.2.3`, not `vv1.2.3`, which is a tag no release carries.
    // spec: CLI-142 STO-76
    #[test]
    fn an_explicit_target_pins_a_single_v_prefixed_release_tag() {
        use self_update::UpdateConfig as _;
        let version = validated_target("v1.2.3").expect("a plain version validates");
        let upd = updater(
            "x86_64-unknown-linux-musl",
            Some(&version),
            download_timeout(),
        )
        .expect("building the updater must not require the network");
        assert_eq!(upd.release_tag(), Some("v1.2.3"));
    }

    // With no explicit target the updater is left unpinned, so the lookup goes to
    // the `releases/latest` endpoint rather than to a tag.
    // spec: CLI-140
    #[test]
    fn no_explicit_target_leaves_the_updater_unpinned() {
        use self_update::UpdateConfig as _;
        let upd = updater("x86_64-unknown-linux-musl", None, lookup_timeout())
            .expect("building the updater must not require the network");
        assert_eq!(upd.release_tag(), None);
        assert_eq!(
            upd.target(),
            "x86_64-unknown-linux-musl",
            "the resolved triple selects the release asset (STO-65)"
        );
    }

    // spec: CLI-217
    #[test]
    fn download_banner_is_suppressed_under_json() {
        // `evolve` writes its own document straight to stdout, so it is exempt from
        // CLI-217's structural fd-1 protection and this guard is the only thing
        // keeping the progress line out of a `--json` run's single document. The
        // text-mode branch is asserted too, so deleting the line outright rather
        // than routing it fails here instead of passing by coincidence.
        assert_eq!(
            download_banner(true, "1.2.3", "x86_64-unknown-linux-musl"),
            None,
            "--json must emit no progress line"
        );
        let line = download_banner(false, "1.2.3", "x86_64-unknown-linux-musl")
            .expect("text mode must still announce the download");
        assert!(
            line.contains("downloading mind 1.2.3"),
            "the banner must name the version being installed: {line}"
        );
        assert!(
            line.contains("x86_64-unknown-linux-musl"),
            "the banner must name the artifact's target triple: {line}"
        );
    }

    // spec: TUI-61
    #[test]
    fn mktemp_dir_prefixed_creates_a_fresh_directory() {
        // The directory must exist after the call returns and must be empty.
        let dir = mktemp_dir_prefixed("mind-test").expect("mktemp_dir_prefixed");
        let exists = dir.is_dir();
        let named = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("mind-test-"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(exists, "the directory must be created: {dir:?}");
        assert!(
            named,
            "the caller's prefix must name the directory: {dir:?}"
        );
    }

    // spec: TUI-61
    #[test]
    fn mktemp_dir_prefixed_yields_distinct_paths() {
        // Two successive calls must return different paths (the sequence number
        // component guarantees this within a process), and both must be creatable
        // -- proving the exclusive-create semantics would reject a pre-existing dir.
        let a = mktemp_dir_prefixed("mind-test").expect("first mktemp_dir_prefixed");
        let b = mktemp_dir_prefixed("mind-test").expect("second mktemp_dir_prefixed");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
        assert_ne!(a, b, "successive calls must yield distinct paths");
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
