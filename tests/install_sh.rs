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

/// A fake `uname` reporting Darwin/arm64 (Apple Silicon), the one macOS
/// combination install.sh publishes a binary for.
fn write_fake_uname_darwin_arm64(bin_dir: &Path) {
    write_exec(
        &bin_dir.join("uname"),
        "#!/bin/sh\ncase \"$1\" in\n-s) echo Darwin ;;\n-m) echo arm64 ;;\nesac\n",
    );
}

/// A fake `gh` that records every invocation's argv to `log_path` and exits
/// with `exit_code`. The recording is what proves the stub -- and not this
/// machine's real, network-capable `gh` -- is what install.sh actually ran.
fn write_fake_gh_logging(bin_dir: &Path, log_path: &Path, exit_code: i32) {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log:?}\nexit {exit_code}\n",
        log = log_path,
    );
    write_exec(&bin_dir.join("gh"), &script);
}

/// Like [`run_install_sh`] but pins `MIND_VERSION` instead of removing it, so
/// the `releases/latest` lookup is skipped entirely.
fn run_install_sh_pinned(
    bin_dir: &Path,
    install_dir: &Path,
    version: &str,
) -> std::process::Output {
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{orig_path}", bin_dir.display());
    Command::new("sh")
        .arg(install_sh_path())
        .env("PATH", new_path)
        .env("MIND_INSTALL_DIR", install_dir)
        .env("MIND_VERSION", version)
        .output()
        .expect("spawn sh resources/install.sh")
}

/// Resolve `tool` against the ambient `PATH`, for building a restricted `PATH`
/// that contains only an explicit tool list (used to make `curl` genuinely
/// absent so the `wget` branch is reachable).
fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let candidate = Path::new(dir).join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Symlink each named tool from the ambient `PATH` into `bin_dir`, so a `PATH`
/// consisting of `bin_dir` alone still resolves the real utilities install.sh
/// shells out to -- and nothing else. Anything not listed (notably `curl` and
/// the real `gh`) is unreachable.
fn link_real_tools(bin_dir: &Path, tools: &[&str]) {
    for tool in tools {
        let real = which(tool).unwrap_or_else(|| panic!("{tool} must exist on PATH for this test"));
        let dest = bin_dir.join(tool);
        if dest.exists() {
            continue;
        }
        std::os::unix::fs::symlink(&real, &dest)
            .unwrap_or_else(|e| panic!("symlink {real:?} -> {dest:?}: {e}"));
    }
}

/// A fake `wget` mirroring [`write_fake_curl`]'s canned-response behavior for
/// wget's argument shape (`-qO-` to stdout, `-qO <dest>` to a file). Exits 8
/// (wget's "server issued an error response") when the asset is absent.
fn write_fake_wget(bin_dir: &Path, fixture_dir: &Path, log_path: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         LOG={log:?}\n\
         RELEASE_JSON={release:?}\n\
         SUMS={sums:?}\n\
         ASSETS_DIR={assets:?}\n\
         \n\
         url=\"\"\n\
         dest=\"\"\n\
         tostdout=0\n\
         prev=\"\"\n\
         for a in \"$@\"; do\n\
         \tcase \"$a\" in\n\
         \t-qO-) tostdout=1 ;;\n\
         \thttps://*) url=\"$a\" ;;\n\
         \tesac\n\
         \tif [ \"$prev\" = \"-qO\" ]; then\n\
         \t\tdest=\"$a\"\n\
         \tfi\n\
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
         \tif [ \"$tostdout\" = 1 ]; then cat \"$SUMS\"; else cp \"$SUMS\" \"$dest\"; fi\n\
         \texit 0\n\
         \t;;\n\
         *)\n\
         \tfname=${{url##*/}}\n\
         \tif [ -f \"$ASSETS_DIR/$fname\" ]; then\n\
         \t\tif [ \"$tostdout\" = 1 ]; then cat \"$ASSETS_DIR/$fname\"; else cp \"$ASSETS_DIR/$fname\" \"$dest\"; fi\n\
         \t\texit 0\n\
         \telse\n\
         \t\texit 8\n\
         \tfi\n\
         \t;;\n\
         esac\n",
        log = log_path,
        release = fixture_dir.join("release.json"),
        sums = fixture_dir.join("sums"),
        assets = fixture_dir.join("assets"),
    );
    write_exec(&bin_dir.join("wget"), &script);
}

/// The real utilities install.sh shells out to, for the restricted-`PATH` run.
/// Deliberately excludes `curl` and `gh`.
const REAL_TOOLS: &[&str] = &[
    "sed",
    "head",
    "mktemp",
    "awk",
    "sha256sum",
    "tar",
    "mkdir",
    "cp",
    "chmod",
    "rm",
    "cat",
    "install",
    // `tar -xzf` forks gzip.
    "gzip",
    // The fake curl script's asset-matching arm shells out to `basename`.
    "basename",
];

/// A per-test fixture: a scratch dir with `bin/`, `assets/`, a request log, and
/// a canned `release.json` naming `version`.
struct Fixture {
    dir: PathBuf,
    bin_dir: PathBuf,
    assets_dir: PathBuf,
    log_path: PathBuf,
    install_dir: PathBuf,
    version: String,
}

impl Fixture {
    fn new(tag: &str, version: &str) -> Fixture {
        let dir = scratch_dir(tag);
        let bin_dir = dir.join("bin");
        let assets_dir = dir.join("assets");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(&assets_dir).unwrap();
        std::fs::write(
            dir.join("release.json"),
            format!(r#"{{"tag_name": "v{version}"}}"#),
        )
        .unwrap();
        Fixture {
            log_path: dir.join("urls.log"),
            install_dir: dir.join("install"),
            bin_dir,
            assets_dir,
            version: version.to_string(),
            dir,
        }
    }

    fn asset_name(&self, triple: &str) -> String {
        format!("mind-{}-{triple}.tar.gz", self.version)
    }

    /// Package an asset for `triple` into `assets/` and return its name.
    fn publish(&self, triple: &str, payload: &[u8]) -> String {
        let name = self.asset_name(triple);
        build_asset_tarball(&self.assets_dir.join(&name), payload);
        name
    }

    /// Write a `SHA256SUMS` fixture listing each named asset's real digest.
    fn publish_sums(&self, names: &[&str]) {
        let mut body = String::new();
        for name in names {
            let digest = sha256_of(&self.assets_dir.join(name));
            body.push_str(&format!("{digest}  {name}\n"));
        }
        std::fs::write(self.dir.join("sums"), body).unwrap();
    }

    /// Write a `SHA256SUMS` fixture with explicit (possibly wrong) digests.
    fn publish_sums_raw(&self, entries: &[(&str, &str)]) {
        let mut body = String::new();
        for (digest, name) in entries {
            body.push_str(&format!("{digest}  {name}\n"));
        }
        std::fs::write(self.dir.join("sums"), body).unwrap();
    }

    fn requested(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

#[test]
// spec: STO-71
fn install_sh_fallback_note_names_the_version_and_the_gnu_retry() {
    // STO-71 requires the fallback to "print a note naming the version and the
    // fallback before retrying". Neither original test asserted the note at
    // all, so its wording (the only signal a user gets that the leg changed)
    // was unverified.
    let fx = Fixture::new("fallback-note", "9.9.9");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("x86_64-unknown-linux-gnu", b"GNU");
    fx.publish_sums(&[&gnu]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        out.status.success(),
        "install must succeed via the fallback"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("musl asset unavailable for 9.9.9, retrying with gnu"),
        "the fallback note must name both the resolved version and the leg it \
         retries with: {stdout}"
    );
}

#[test]
// spec: STO-71
fn install_sh_fallback_fires_exactly_once_and_never_loops() {
    // STO-71: "The fallback triggers only on Linux and only when the failed
    // asset was the musl one (so it fires exactly once, never loops)". With
    // BOTH legs absent, the script must make exactly two asset requests and
    // then die -- not retry the gnu leg repeatedly, and not fall back from gnu
    // to anything else.
    let fx = Fixture::new("fallback-once", "9.9.9");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);
    // No assets published at all, and no SHA256SUMS is ever reached.
    std::fs::write(fx.dir.join("sums"), "").unwrap();

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        !out.status.success(),
        "with neither leg published the install must fail: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let musl = fx.asset_name("x86_64-unknown-linux-musl");
    let gnu = fx.asset_name("x86_64-unknown-linux-gnu");
    let log = fx.requested();
    let asset_requests: Vec<&str> = log.lines().filter(|l| l.contains(".tar.gz")).collect();
    assert_eq!(
        asset_requests.len(),
        2,
        "exactly two asset requests (musl, then one gnu retry) -- no loop: {log:?}"
    );
    assert!(
        asset_requests[0].contains(&musl),
        "the first request must be the musl leg: {log:?}"
    );
    assert!(
        asset_requests[1].contains(&gnu),
        "the single retry must be the gnu leg: {log:?}"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("download failed") && stderr.contains(&gnu),
        "the final error must name the gnu URL that also failed, not the \
         original musl one: {stderr}"
    );
    assert!(
        !fx.install_dir.join("mind").exists(),
        "nothing may be installed when every leg 404s"
    );
}

#[test]
// spec: STO-71
fn install_sh_does_not_fall_back_on_macos() {
    // STO-71 scopes the fallback to Linux. On Darwin a failed download must be
    // a hard error with no second request: there is no `aarch64-apple-darwin`
    // gnu leg to retry, and requesting one would be a guaranteed second 404.
    let fx = Fixture::new("no-fallback-darwin", "9.9.9");
    write_fake_uname_darwin_arm64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);
    std::fs::write(fx.dir.join("sums"), "").unwrap();

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        !out.status.success(),
        "a 404 on macOS must be fatal: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let log = fx.requested();
    let asset_requests: Vec<&str> = log.lines().filter(|l| l.contains(".tar.gz")).collect();
    assert_eq!(
        asset_requests.len(),
        1,
        "macOS must make exactly one asset request and stop: {log:?}"
    );
    assert!(
        asset_requests[0].contains(&fx.asset_name("aarch64-apple-darwin")),
        "the single request must be the darwin asset: {log:?}"
    );
    assert!(
        !log.contains("linux-gnu"),
        "the Linux gnu fallback must never fire on macOS: {log:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("retrying with gnu"),
        "the fallback note must not be printed on macOS: {stdout}"
    );
}

#[test]
// spec: STO-71
fn install_sh_verifies_the_checksum_of_the_asset_it_actually_downloaded() {
    // The fallback rebinds `asset`; if it did not, the SHA256SUMS lookup and
    // the digest comparison would be done against the musl name while the gnu
    // bytes sat on disk. Publish a gnu asset whose SHA256SUMS entry is WRONG:
    // the install must abort on a checksum mismatch naming the gnu asset, and
    // install nothing.
    let fx = Fixture::new("fallback-checksum", "9.9.9");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("x86_64-unknown-linux-gnu", b"GNU-TAMPERED");
    fx.publish_sums_raw(&[(
        "0000000000000000000000000000000000000000000000000000000000000000",
        &gnu,
    )]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        !out.status.success(),
        "a checksum mismatch on the fallback asset must be fatal: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("checksum mismatch") && stderr.contains(&gnu),
        "the failure must be a checksum mismatch naming the gnu asset that was \
         actually downloaded: {stderr}"
    );
    assert!(
        !fx.install_dir.join("mind").exists(),
        "a checksum mismatch must install nothing"
    );
}

#[test]
// spec: STO-71
fn install_sh_fallback_requires_a_sha256sums_entry_for_the_gnu_asset() {
    // The complementary half of the rebinding check: SHA256SUMS lists only the
    // musl entry. Because `asset` was rebound to the gnu name, the exact-match
    // awk lookup finds nothing and the install aborts naming the gnu asset. If
    // `asset` had NOT been rebound, the musl entry would be found and compared
    // against the gnu bytes -- a mismatch, but for the wrong reason, and a
    // deliberately substituted musl entry would validate gnu bytes.
    let fx = Fixture::new("fallback-sums-entry", "9.9.9");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("x86_64-unknown-linux-gnu", b"GNU");
    let gnu_digest = sha256_of(&fx.assets_dir.join(&gnu));
    let musl = fx.asset_name("x86_64-unknown-linux-musl");
    // Only a musl row, carrying the GNU payload's real digest: the only way
    // this install can succeed is by looking the digest up under the stale
    // musl name.
    fx.publish_sums_raw(&[(&gnu_digest, &musl)]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        !out.status.success(),
        "the digest must be looked up under the gnu asset name: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("SHA256SUMS has no entry for") && stderr.contains(&gnu),
        "the failure must name the missing gnu entry: {stderr}"
    );
    assert!(
        !fx.install_dir.join("mind").exists(),
        "nothing may be installed when the digest cannot be looked up"
    );
}

#[test]
// spec: STO-71
fn install_sh_pinned_version_skips_the_release_lookup_and_still_falls_back() {
    // `MIND_VERSION` short-circuits the `releases/latest` request, so the whole
    // "served from main, resolved from the latest release" rationale is
    // bypassed -- but the fallback must still work, because a pinned OLD
    // version is precisely the case where the musl leg is missing.
    let fx = Fixture::new("pinned-fallback", "0.21.0");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("x86_64-unknown-linux-gnu", b"OLD-GNU");
    fx.publish_sums(&[&gnu]);

    let out = run_install_sh_pinned(&fx.bin_dir, &fx.install_dir, "0.21.0");
    assert!(
        out.status.success(),
        "a pinned version must still reach the gnu fallback\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = fx.requested();
    assert!(
        !log.contains("releases/latest"),
        "MIND_VERSION must suppress the latest-release lookup entirely: {log:?}"
    );
    assert_eq!(
        std::fs::read(fx.install_dir.join("mind")).unwrap(),
        b"OLD-GNU",
        "the pinned version's gnu payload must be what lands on disk"
    );
}

#[test]
fn install_sh_attestation_check_runs_the_stubbed_gh_not_the_real_one() {
    // The `gh` stub is what keeps this suite network-free: install.sh's
    // provenance step shells out to `gh attestation verify`, which on a
    // developer machine with a real `gh` installed would reach GitHub. Record
    // the stub's argv and assert install.sh actually invoked THAT binary, with
    // the downloaded tarball and the jaemk/mind repo -- so a future PATH
    // change that let the real `gh` through is caught here.
    let fx = Fixture::new("gh-stub-fails", "9.9.9");
    let gh_log = fx.dir.join("gh.log");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_logging(&fx.bin_dir, &gh_log, 1);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let musl = fx.publish("x86_64-unknown-linux-musl", b"MUSL");
    fx.publish_sums(&[&musl]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        out.status.success(),
        "a failed provenance check is soft and must not fail the install: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let recorded = std::fs::read_to_string(&gh_log)
        .expect("install.sh must have invoked the stubbed gh, not the real one");
    assert!(
        recorded.contains("attestation verify"),
        "the stub must have received the attestation subcommand: {recorded:?}"
    );
    assert!(
        recorded.contains("--repo jaemk/mind"),
        "the stub must have received the repo argument: {recorded:?}"
    );
    assert!(
        recorded.contains(&musl),
        "the stub must have been handed the downloaded tarball: {recorded:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("build provenance could not be verified (continuing)"),
        "a failed soft check must say so on stderr and continue: {stderr}"
    );
}

#[test]
fn install_sh_reports_verified_provenance_when_gh_succeeds() {
    // The other side of the soft check: a `gh` that exits 0 produces the
    // positive note. Only the failing branch was exercised before, so a
    // wording or logic inversion in the success arm was invisible.
    let fx = Fixture::new("gh-stub-ok", "9.9.9");
    let gh_log = fx.dir.join("gh.log");
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_logging(&fx.bin_dir, &gh_log, 0);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let musl = fx.publish("x86_64-unknown-linux-musl", b"MUSL");
    fx.publish_sums(&[&musl]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(out.status.success(), "install must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("build provenance verified"),
        "a successful soft check must report it: {stdout}"
    );
    assert!(
        !stdout.contains("could not be verified"),
        "the failure note must not also appear: {stdout}"
    );
}

#[test]
// spec: STO-71
fn install_sh_falls_back_over_wget_when_curl_is_absent() {
    // install.sh's downloader is curl-or-wget, but every existing test stubs
    // curl, so the wget arm of `fetch`/`fetch_to` -- and therefore the whole
    // fallback path over wget -- had never run. `PATH` here is a restricted
    // directory holding only the real utilities install.sh needs plus fake
    // `uname`/`wget`: `curl` and `gh` are genuinely absent, which also proves
    // the run cannot reach the network by either route.
    let fx = Fixture::new("wget-fallback", "9.9.9");
    link_real_tools(&fx.bin_dir, REAL_TOOLS);
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_wget(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("x86_64-unknown-linux-gnu", b"WGET-GNU");
    fx.publish_sums(&[&gnu]);

    let sh = which("sh").expect("sh must exist on PATH");
    let out = Command::new(&sh)
        .arg(install_sh_path())
        .env("PATH", fx.bin_dir.display().to_string())
        .env("MIND_INSTALL_DIR", &fx.install_dir)
        .env_remove("MIND_VERSION")
        .output()
        .expect("spawn sh resources/install.sh under a restricted PATH");
    assert!(
        out.status.success(),
        "install.sh must work with wget only\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(fx.install_dir.join("mind")).unwrap(),
        b"WGET-GNU",
        "the gnu payload fetched over wget must land on disk"
    );

    let log = fx.requested();
    assert!(
        log.contains("releases/latest"),
        "the version lookup must have gone through the wget arm of fetch(): {log:?}"
    );
    assert!(
        log.contains(&fx.asset_name("x86_64-unknown-linux-musl")),
        "the musl leg must be attempted first over wget too: {log:?}"
    );
    assert!(
        log.contains(&gnu),
        "the gnu retry must happen over wget too: {log:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("build provenance"),
        "with no gh on PATH the provenance step must be skipped silently: {stdout}"
    );
}

#[test]
// spec: STO-74
fn install_sh_fetch_to_refuses_without_curl_or_wget() {
    // `fetch` (used for the `releases/latest` lookup) already refuses cleanly
    // when neither curl nor wget is on PATH. `fetch_to` (used for the asset and
    // SHA256SUMS downloads) is the one that mattered less obviously: it is
    // reachable WITHOUT going through `fetch` at all when `MIND_VERSION` is
    // set, since that short-circuits the `releases/latest` lookup entirely.
    // `PATH` here is a restricted directory holding only the real utilities
    // install.sh needs plus a fake `uname`: curl, wget, and gh are genuinely
    // absent, so this run provably cannot reach the network by any route, and
    // `fetch_to` is the first (and only) downloader call.
    let fx = Fixture::new("no-downloader", "9.9.9");
    link_real_tools(&fx.bin_dir, REAL_TOOLS);
    write_fake_uname_linux_x86_64(&fx.bin_dir);

    let sh = which("sh").expect("sh must exist on PATH");
    let out = Command::new(&sh)
        .arg(install_sh_path())
        .env("PATH", fx.bin_dir.display().to_string())
        .env("MIND_INSTALL_DIR", &fx.install_dir)
        .env("MIND_VERSION", "9.9.9")
        .output()
        .expect("spawn sh resources/install.sh under a restricted PATH");

    assert!(
        !out.status.success(),
        "install.sh must fail with no downloader on PATH"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("need curl or wget on PATH"),
        "fetch_to must refuse with the same message fetch() uses, not an \
         opaque download failure: stderr: {stderr}"
    );
    assert!(
        !stderr.contains("download failed:"),
        "the real cause (no downloader) must not be masked by a generic \
         download-failed message: stderr: {stderr}"
    );
    assert!(
        !fx.install_dir.join("mind").exists(),
        "nothing must be installed when there is no downloader"
    );
}

#[test]
// spec: STO-74
fn install_sh_fetch_refuses_without_curl_or_wget_when_version_is_not_pinned() {
    // The counterpart to `install_sh_fetch_to_refuses_without_curl_or_wget`:
    // that test only ever exercises `fetch_to`'s missing-downloader arm,
    // because it pins `MIND_VERSION` so `fetch`'s own `releases/latest` lookup
    // is skipped. Nothing in this suite -- before or after the STO-74 fix --
    // ever ran `fetch`'s pre-existing `else err "need curl or wget on PATH"`
    // arm (the one `fetch_to`'s new arm was written to match). Same restricted,
    // downloader-less `PATH` as the `fetch_to` test, but `MIND_VERSION` is left
    // unset so `fetch` is reached first, exactly as an un-pinned install would.
    //
    // Unlike `fetch_to` (called directly in a top-level `if`), `fetch` is
    // called inside a pipeline within a command substitution
    // (`tag="$(fetch ... | sed ... | head -n 1)"`), and each pipeline stage in
    // `sh`/`dash` runs in its own subshell. So `err`'s `exit 1` only kills
    // that subshell, not the main script: the real-cause message still prints
    // first, but the script then falls through to `[ -n "$tag" ] || err
    // "could not determine the latest release; set MIND_VERSION"`, so a
    // SECOND, derived error line trails it before the script actually exits.
    // This is pre-existing `fetch`/version-resolution plumbing the STO-74 diff
    // does not touch, so it is asserted here as documented behavior (the true
    // cause still prints, first and un-swallowed) rather than tightened.
    let fx = Fixture::new("no-downloader-unpinned", "9.9.9");
    link_real_tools(&fx.bin_dir, REAL_TOOLS);
    write_fake_uname_linux_x86_64(&fx.bin_dir);

    let sh = which("sh").expect("sh must exist on PATH");
    let out = Command::new(&sh)
        .arg(install_sh_path())
        .env("PATH", fx.bin_dir.display().to_string())
        .env("MIND_INSTALL_DIR", &fx.install_dir)
        .env_remove("MIND_VERSION")
        .output()
        .expect("spawn sh resources/install.sh under a restricted PATH");

    assert!(
        !out.status.success(),
        "install.sh must fail with no downloader on PATH and no pinned version"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first_line = stderr.lines().next().unwrap_or_default();
    assert!(
        first_line.contains("need curl or wget on PATH"),
        "the real cause must be the FIRST thing printed, not buried after a \
         derived error: stderr: {stderr}"
    );
    assert!(
        !stderr.contains("download failed:"),
        "the real cause (no downloader) must never be masked by the generic \
         download-failed wording: stderr: {stderr}"
    );
    assert_eq!(
        fx.requested(),
        "",
        "no request can have been logged: there is no curl or wget stub to log \
         one, so any content here would mean install.sh reached a real \
         downloader"
    );
    assert!(
        !fx.install_dir.join("mind").exists(),
        "nothing must be installed when there is no downloader"
    );
}

#[test]
fn install_sh_prefers_curl_over_wget_when_both_are_present() {
    // `fetch`/`fetch_to` both pick curl first (`if command -v curl ... elif
    // command -v wget ...`), but no existing test puts BOTH a working curl and
    // a working wget on `PATH` at once: every curl-path test relies on
    // whatever wget may or may not be reachable further down the *inherited*
    // PATH (never asserted either way), and the dedicated wget test makes curl
    // genuinely absent rather than present-but-unused. Here both are stubbed,
    // each logging to its OWN file, on a restricted, isolated PATH: if the
    // preference ever flipped (or a future refactor tried both, or fell
    // through to wget for the asset/sums calls even though curl succeeded for
    // the version lookup), the wget log would be non-empty.
    let fx = Fixture::new("both-present", "9.9.9");
    link_real_tools(&fx.bin_dir, REAL_TOOLS);
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);
    let wget_log = fx.dir.join("wget_urls.log");
    write_fake_wget(&fx.bin_dir, &fx.dir, &wget_log);

    let musl = fx.publish("x86_64-unknown-linux-musl", b"BOTH-PRESENT-MUSL");
    fx.publish_sums(&[&musl]);

    let sh = which("sh").expect("sh must exist on PATH");
    let out = Command::new(&sh)
        .arg(install_sh_path())
        .env("PATH", fx.bin_dir.display().to_string())
        .env("MIND_INSTALL_DIR", &fx.install_dir)
        .env_remove("MIND_VERSION")
        .output()
        .expect("spawn sh resources/install.sh under a restricted PATH");

    assert!(
        out.status.success(),
        "install must succeed with both downloaders present\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(fx.install_dir.join("mind")).unwrap(),
        b"BOTH-PRESENT-MUSL",
        "the payload fetched over curl must land on disk"
    );

    let curl_log = fx.requested();
    assert!(
        curl_log.contains("releases/latest"),
        "the version lookup (fetch) must go over curl: {curl_log:?}"
    );
    assert!(
        curl_log.contains(&musl),
        "the asset download (fetch_to) must go over curl: {curl_log:?}"
    );
    assert!(
        curl_log.contains("SHA256SUMS"),
        "the SHA256SUMS download (fetch_to) must go over curl: {curl_log:?}"
    );
    assert!(
        !wget_log.exists(),
        "wget must never be invoked when curl is present on PATH: found {wget_log:?}"
    );
}

#[test]
fn install_sh_uses_curl_when_wget_is_genuinely_absent() {
    // The mirror image of `install_sh_falls_back_over_wget_when_curl_is_absent`:
    // that test proves the wget arm works with curl genuinely absent. This
    // proves the curl arm works with wget genuinely absent -- every OTHER
    // curl-path test in this suite prepends the stub dir to the inherited
    // PATH, so real wget's presence or absence is never controlled. Here PATH
    // is exactly the restricted directory: wget is not on it at all.
    let fx = Fixture::new("curl-only", "9.9.9");
    link_real_tools(&fx.bin_dir, REAL_TOOLS);
    write_fake_uname_linux_x86_64(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let musl = fx.publish("x86_64-unknown-linux-musl", b"CURL-ONLY-MUSL");
    fx.publish_sums(&[&musl]);

    let sh = which("sh").expect("sh must exist on PATH");
    let out = Command::new(&sh)
        .arg(install_sh_path())
        .env("PATH", fx.bin_dir.display().to_string())
        .env("MIND_INSTALL_DIR", &fx.install_dir)
        .env_remove("MIND_VERSION")
        .output()
        .expect("spawn sh resources/install.sh under a restricted PATH");

    assert!(
        out.status.success(),
        "install.sh must work with curl only\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(fx.install_dir.join("mind")).unwrap(),
        b"CURL-ONLY-MUSL",
        "the payload fetched over curl must land on disk"
    );

    let log = fx.requested();
    assert!(
        log.contains("releases/latest"),
        "the version lookup must have gone through the curl arm of fetch(): {log:?}"
    );
    assert!(
        log.contains(&musl),
        "the asset download must have gone through the curl arm of fetch_to(): {log:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("build provenance"),
        "with no gh on PATH the provenance step must be skipped silently: {stdout}"
    );
}

#[test]
fn install_sh_rejects_an_unsupported_os_and_arch_before_any_request() {
    // The two hard-refusal arms of the target-triple resolution. Both must
    // fire before a single network request is made.
    for (os, arch, needle) in [
        ("Plan9", "x86_64", "unsupported OS"),
        ("Linux", "riscv64", "unsupported architecture"),
    ] {
        let fx = Fixture::new("unsupported", "9.9.9");
        write_exec(
            &fx.bin_dir.join("uname"),
            &format!("#!/bin/sh\ncase \"$1\" in\n-s) echo {os} ;;\n-m) echo {arch} ;;\nesac\n"),
        );
        write_fake_gh_always_fails(&fx.bin_dir);
        write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

        let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
        assert!(!out.status.success(), "{os}/{arch} must be refused");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(needle),
            "{os}/{arch} must be refused with '{needle}': {stderr}"
        );
        assert_eq!(
            fx.requested(),
            "",
            "the refusal must happen before any request for {os}/{arch}"
        );
    }
}

#[test]
fn install_sh_refuses_intel_macos_before_any_request() {
    // No Intel macOS binary is published, so the script refuses rather than
    // requesting an asset that can never exist.
    let fx = Fixture::new("intel-mac", "9.9.9");
    write_exec(
        &fx.bin_dir.join("uname"),
        "#!/bin/sh\ncase \"$1\" in\n-s) echo Darwin ;;\n-m) echo x86_64 ;;\nesac\n",
    );
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(!out.status.success(), "Intel macOS must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no prebuilt binary for Intel macOS"),
        "the refusal must name the reason: {stderr}"
    );
    assert_eq!(
        fx.requested(),
        "",
        "the refusal must happen before any request"
    );
}

#[test]
// spec: STO-71
fn install_sh_aarch64_linux_prefers_musl_and_falls_back_to_its_own_arch_gnu() {
    // The fallback rebuilds the triple from `arch_part`, so on aarch64 it must
    // retry `aarch64-unknown-linux-gnu` -- not the hard-coded x86_64 leg the
    // other tests all exercise.
    let fx = Fixture::new("aarch64-fallback", "9.9.9");
    write_exec(
        &fx.bin_dir.join("uname"),
        "#!/bin/sh\ncase \"$1\" in\n-s) echo Linux ;;\n-m) echo aarch64 ;;\nesac\n",
    );
    write_fake_gh_always_fails(&fx.bin_dir);
    write_fake_curl(&fx.bin_dir, &fx.dir, &fx.log_path);

    let gnu = fx.publish("aarch64-unknown-linux-gnu", b"ARM-GNU");
    fx.publish_sums(&[&gnu]);

    let out = run_install_sh(&fx.bin_dir, &fx.install_dir);
    assert!(
        out.status.success(),
        "aarch64 must reach its own gnu leg\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = fx.requested();
    assert!(
        log.contains(&fx.asset_name("aarch64-unknown-linux-musl")),
        "the aarch64 musl leg must be tried first: {log:?}"
    );
    assert!(
        log.contains(&gnu),
        "the retry must use the aarch64 gnu leg: {log:?}"
    );
    assert!(
        !log.contains("x86_64"),
        "an aarch64 host must never request an x86_64 asset: {log:?}"
    );
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
