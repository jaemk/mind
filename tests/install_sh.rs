//! Hermetic tests for `resources/install.sh`, the Linux/macOS one-line
//! installer. No network: `curl`, `uname`, and `gh` are stubbed by putting
//! fake scripts earlier on `PATH` (the fake-curl pattern established for
//! `evolve` in `src/selfupdate.rs`), while the rest of `PATH` still resolves
//! to the real `sh`/`tar`/`sha256sum`/`mkdir`/`cp`/`chmod`/`install`/
//! `basename`/`cat`/`mktemp` utilities install.sh itself shells out to.
//!
//! Covers the C30/P1 fix: install.sh unconditionally requested the musl
//! Linux asset, which no published release up to and including v0.21.0
//! carries (confirmed via `git show v0.21.0:.github/workflows/release.yml`),
//! so every Linux install 404'd and died. install.sh now falls back to the
//! gnu leg, which every release publishes, when the musl asset 404s.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn install_sh_path() -> PathBuf {
    repo_root().join("resources/install.sh")
}

fn release_yml_path() -> PathBuf {
    repo_root().join(".github/workflows/release.yml")
}

/// A fresh, uniquely-named scratch directory under the system temp dir,
/// mirroring the `mind-it-<pid>-<n>` convention `tests/cli.rs` uses.
fn scratch_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("mind-install-sh-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_exec(path: &Path, content: &str) {
    std::fs::write(path, content).expect("write script");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).expect("stat script").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod script");
}

/// Package `content` as an executable `mind` binary inside a `.tar.gz`, via
/// the real `tar` binary (mirrors what the release workflow packages; purely
/// local filesystem work, no network).
fn build_asset_tarball(out_path: &Path, content: &[u8]) {
    let staging = scratch_dir("pkg");
    let bin = staging.join("mind");
    std::fs::write(&bin, content).expect("write mind binary");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&bin)
        .expect("stat staged binary")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).expect("chmod staged binary");

    let status = Command::new("tar")
        .arg("-czf")
        .arg(out_path)
        .arg("-C")
        .arg(&staging)
        .arg("mind")
        .status()
        .expect("spawn tar");
    assert!(status.success(), "tar packaging must succeed");
    let _ = std::fs::remove_dir_all(&staging);
}

fn sha256_of(path: &Path) -> String {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("spawn sha256sum");
    assert!(out.status.success(), "sha256sum must succeed on {path:?}");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum output must start with the digest")
        .to_string()
}

/// Write a fake `curl` that serves canned responses out of `fixture_dir` and
/// appends every requested URL to `log_path` (one per line), never touching
/// the network:
/// - `.../releases/latest`   -> cats `fixture_dir/release.json` to stdout
/// - `...SHA256SUMS`         -> copies `fixture_dir/sums` to the `-o` dest
/// - anything else           -> copies `fixture_dir/assets/<basename>` to the
///   `-o` dest if it exists, else exits 22 (curl's `-f` HTTP-error exit
///   code), simulating a 404 for whichever asset the fixture omits.
fn write_fake_curl(bin_dir: &Path, fixture_dir: &Path, log_path: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         LOG={log:?}\n\
         RELEASE_JSON={release:?}\n\
         SUMS={sums:?}\n\
         ASSETS_DIR={assets:?}\n\
         \n\
         url=\"\"\n\
         dest=\"\"\n\
         prev=\"\"\n\
         for a in \"$@\"; do\n\
         \tif [ \"$prev\" = \"-o\" ]; then\n\
         \t\tdest=\"$a\"\n\
         \tfi\n\
         \tcase \"$a\" in\n\
         \thttps://*) url=\"$a\" ;;\n\
         \tesac\n\
         \tprev=\"$a\"\n\
         done\n\
         printf '%s\\n' \"$url\" >> \"$LOG\"\n\
         \n\
         case \"$url\" in\n\
         */releases/latest)\n\
         \tcat \"$RELEASE_JSON\"\n\
         \texit 0\n\
         \t;;\n\
         *SHA256SUMS)\n\
         \tcp \"$SUMS\" \"$dest\"\n\
         \texit 0\n\
         \t;;\n\
         *)\n\
         \tfname=$(basename \"$url\")\n\
         \tif [ -f \"$ASSETS_DIR/$fname\" ]; then\n\
         \t\tcp \"$ASSETS_DIR/$fname\" \"$dest\"\n\
         \t\texit 0\n\
         \telse\n\
         \t\texit 22\n\
         \tfi\n\
         \t;;\n\
         esac\n",
        log = log_path,
        release = fixture_dir.join("release.json"),
        sums = fixture_dir.join("sums"),
        assets = fixture_dir.join("assets"),
    );
    write_exec(&bin_dir.join("curl"), &script);
}

/// A fake `uname` reporting Linux/x86_64, matching the `os`/`arch` values
/// install.sh feeds into its target-triple resolution.
fn write_fake_uname_linux_x86_64(bin_dir: &Path) {
    write_exec(
        &bin_dir.join("uname"),
        "#!/bin/sh\ncase \"$1\" in\n-s) echo Linux ;;\n-m) echo x86_64 ;;\nesac\n",
    );
}

/// A fake `gh` that always fails without touching the network. install.sh's
/// attestation check is soft (`if gh attestation verify ...; then ... else
/// ... fi`), so this deterministically exercises the "could not be verified,
/// continuing" branch instead of shelling out to a real `gh` that may be
/// installed on the machine running the test.
fn write_fake_gh_always_fails(bin_dir: &Path) {
    write_exec(&bin_dir.join("gh"), "#!/bin/sh\nexit 1\n");
}

/// Run `sh resources/install.sh` with `bin_dir` shadowing `curl`/`uname`/`gh`
/// at the FRONT of `PATH` (the rest of the real `PATH` stays available so
/// `mktemp`/`tar`/`sha256sum`/`mkdir`/`cp`/`chmod`/`install`/`basename`/`cat`
/// still resolve), installing into `install_dir`.
fn run_install_sh(bin_dir: &Path, install_dir: &Path) -> std::process::Output {
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{orig_path}", bin_dir.display());
    Command::new("sh")
        .arg(install_sh_path())
        .env("PATH", new_path)
        .env("MIND_INSTALL_DIR", install_dir)
        .env_remove("MIND_VERSION")
        .output()
        .expect("spawn sh resources/install.sh")
}

#[test]
// spec: STO-71
fn install_sh_falls_back_to_gnu_when_musl_asset_404s_on_linux() {
    // Reproduces C30/P1: the musl leg is not published on every release, so
    // the first request 404s; install.sh must retry the gnu leg (which every
    // release carries) rather than dying outright.
    let fixture = scratch_dir("fixture-fallback");
    let bin_dir = fixture.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let assets_dir = fixture.join("assets");
    std::fs::create_dir_all(&assets_dir).unwrap();
    let log_path = fixture.join("urls.log");

    write_fake_uname_linux_x86_64(&bin_dir);
    write_fake_gh_always_fails(&bin_dir);
    write_fake_curl(&bin_dir, &fixture, &log_path);

    let version = "9.9.9";
    std::fs::write(
        fixture.join("release.json"),
        format!(r#"{{"tag_name": "v{version}"}}"#),
    )
    .unwrap();

    let gnu_asset_name = format!("mind-{version}-x86_64-unknown-linux-gnu.tar.gz");
    let musl_asset_name = format!("mind-{version}-x86_64-unknown-linux-musl.tar.gz");
    // The musl asset is intentionally NOT written into assets/, so the fake
    // curl 404s it, exactly like the currently-published releases do.
    let gnu_asset_path = assets_dir.join(&gnu_asset_name);
    build_asset_tarball(&gnu_asset_path, b"GNU-BUILD-CONTENT");
    let gnu_digest = sha256_of(&gnu_asset_path);
    std::fs::write(
        fixture.join("sums"),
        format!("{gnu_digest}  {gnu_asset_name}\n"),
    )
    .unwrap();

    let install_dir = fixture.join("install");
    let out = run_install_sh(&bin_dir, &install_dir);
    assert!(
        out.status.success(),
        "install.sh must succeed via the gnu fallback\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let installed = install_dir.join("mind");
    assert!(
        installed.is_file(),
        "installed binary must exist at {installed:?}"
    );
    let installed_content = std::fs::read(&installed).unwrap();
    assert_eq!(
        installed_content, b"GNU-BUILD-CONTENT",
        "the installed binary must be the gnu asset's payload"
    );

    let log = std::fs::read_to_string(&log_path).unwrap();
    let musl_idx = log
        .find(&musl_asset_name)
        .unwrap_or_else(|| panic!("install.sh must have attempted the musl asset first: {log}"));
    let gnu_idx = log.find(&gnu_asset_name).unwrap_or_else(|| {
        panic!("install.sh must have retried with the gnu asset after the musl 404: {log}")
    });
    assert!(
        musl_idx < gnu_idx,
        "the musl attempt must precede the gnu retry in the request log: {log:?}"
    );
}

#[test]
// spec: STO-71
fn install_sh_uses_musl_directly_when_available_no_gnu_request() {
    // When the musl asset IS published (a future release with the musl leg
    // built), install.sh must not fall back at all: no gnu request should
    // ever be made.
    let fixture = scratch_dir("fixture-musl-ok");
    let bin_dir = fixture.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let assets_dir = fixture.join("assets");
    std::fs::create_dir_all(&assets_dir).unwrap();
    let log_path = fixture.join("urls.log");

    write_fake_uname_linux_x86_64(&bin_dir);
    write_fake_gh_always_fails(&bin_dir);
    write_fake_curl(&bin_dir, &fixture, &log_path);

    let version = "9.9.9";
    std::fs::write(
        fixture.join("release.json"),
        format!(r#"{{"tag_name": "v{version}"}}"#),
    )
    .unwrap();

    let musl_asset_name = format!("mind-{version}-x86_64-unknown-linux-musl.tar.gz");
    let musl_asset_path = assets_dir.join(&musl_asset_name);
    build_asset_tarball(&musl_asset_path, b"MUSL-BUILD-CONTENT");
    let musl_digest = sha256_of(&musl_asset_path);
    std::fs::write(
        fixture.join("sums"),
        format!("{musl_digest}  {musl_asset_name}\n"),
    )
    .unwrap();

    let install_dir = fixture.join("install");
    let out = run_install_sh(&bin_dir, &install_dir);
    assert!(
        out.status.success(),
        "install.sh must succeed via the direct musl download\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let installed = install_dir.join("mind");
    assert!(
        installed.is_file(),
        "installed binary must exist at {installed:?}"
    );
    let installed_content = std::fs::read(&installed).unwrap();
    assert_eq!(
        installed_content, b"MUSL-BUILD-CONTENT",
        "the installed binary must be the musl asset's payload"
    );

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&musl_asset_name),
        "install.sh must have requested the musl asset: {log:?}"
    );
    let gnu_asset_name = format!("mind-{version}-x86_64-unknown-linux-gnu.tar.gz");
    assert!(
        !log.contains(&gnu_asset_name),
        "install.sh must NOT request the gnu asset when the musl asset succeeds: {log:?}"
    );
}

/// Parse a single `LABEL) VAR="value" ;;` case-arm line into `(VAR, value)`.
fn parse_case_assignment(body: &str) -> Option<(String, String)> {
    let (var, rest) = body.split_once('=')?;
    let var = var.trim();
    let rest = rest.trim();
    if var.is_empty() || !var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let rest = rest.strip_prefix('"')?;
    let val = rest.strip_suffix('"')?;
    Some((var.to_string(), val.to_string()))
}

/// Parse the reachable `os_part`/`arch_part` case arms out of install.sh's
/// own text, cross it with the explicit Intel-macOS rejection and any
/// `target="${arch_part}-<suffix>"` assignment beyond the initial formula
/// (a fallback/retry target, e.g. the C30/P1 gnu retry), and return every
/// target triple install.sh can end up requesting from GitHub. Pure text
/// parsing: no subprocess, no network.
fn install_sh_reachable_targets(script: &str) -> BTreeSet<String> {
    let mut os_parts = BTreeSet::new();
    let mut arch_parts = BTreeSet::new();
    for line in script.lines() {
        let line = line.trim();
        let Some(body) = line.strip_suffix(";;") else {
            continue;
        };
        let body = body.trim();
        let Some(paren_idx) = body.find(')') else {
            continue;
        };
        let assign = body[paren_idx + 1..].trim();
        let Some((var, val)) = parse_case_assignment(assign) else {
            continue;
        };
        match var.as_str() {
            "os_part" => {
                os_parts.insert(val);
            }
            "arch_part" => {
                arch_parts.insert(val);
            }
            _ => {}
        }
    }
    assert!(
        !os_parts.is_empty() && !arch_parts.is_empty(),
        "found no os_part/arch_part case arms; install.sh's case-statement \
         structure changed and this parser must be updated: os_parts={os_parts:?} arch_parts={arch_parts:?}"
    );

    let mut targets: BTreeSet<String> = BTreeSet::new();
    for os in &os_parts {
        for arch in &arch_parts {
            targets.insert(format!("{arch}-{os}"));
        }
    }
    // install.sh explicitly refuses Intel macOS (no prebuilt binary is
    // published): that combination is never actually requested.
    if script.contains("\"$os\" = \"Darwin\"") && script.contains("\"$arch_part\" = \"x86_64\"") {
        targets.remove("x86_64-apple-darwin");
    }

    // Any further `target="${arch_part}-<suffix>"` assignment beyond the
    // initial formula (`target="${arch_part}-${os_part}"`) is a fallback/
    // retry target, reachable for every known arch_part.
    for line in script.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("target=\"${arch_part}-") else {
            continue;
        };
        let Some(end) = rest.find('"') else {
            continue;
        };
        let suffix = &rest[..end];
        if suffix == "${os_part}" {
            continue; // the initial formula line, not a fallback
        }
        for arch in &arch_parts {
            targets.insert(format!("{arch}-{suffix}"));
        }
    }

    targets
}

/// Parse the `target:` entries out of the release workflow's build matrix.
fn release_matrix_targets(yaml: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in yaml.lines() {
        if let Some(val) = line.trim().strip_prefix("- target:") {
            let val = val.trim();
            if !val.is_empty() {
                out.insert(val.to_string());
            }
        }
    }
    out
}

#[test]
// spec: STO-71
fn install_sh_reachable_targets_are_all_published_by_the_release_matrix() {
    // This is the general drift guard the C30/P1 bug demonstrates the need
    // for: install.sh's asset requests and the release workflow's actual
    // build matrix can silently diverge (a target renamed or dropped on one
    // side but not the other), and that divergence is a 100%-failure bug that
    // produces no diagnostic beyond a bare "download failed" (curl -fsSL).
    let script = std::fs::read_to_string(install_sh_path()).expect("read resources/install.sh");
    let yaml =
        std::fs::read_to_string(release_yml_path()).expect("read .github/workflows/release.yml");

    let install_targets = install_sh_reachable_targets(&script);
    let matrix_targets = release_matrix_targets(&yaml);
    assert!(
        matrix_targets.len() >= 3,
        "found only {} target: entries in the release matrix; the parser or \
         workflow layout likely changed: {matrix_targets:?}",
        matrix_targets.len()
    );

    // Lock down the exact set install.sh can reach today, so a change to
    // either file's target logic is caught even in the case where it happens
    // to remain a (now smaller, or now different) subset.
    let expected: BTreeSet<String> = [
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        install_targets, expected,
        "install.sh's reachable target-triple set changed; update this test's \
         expectation deliberately if that was intended"
    );

    let missing: Vec<_> = install_targets
        .difference(&matrix_targets)
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "install.sh can request target triple(s) {missing:?} that the release \
         build matrix (.github/workflows/release.yml) does not publish -- this \
         is exactly the C30/P1 class of drift (the release asset set and \
         install.sh's requests silently diverging)"
    );
}
