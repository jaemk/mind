//! Offline exercises of `resources/publish-release.sh` (STO-91): `git` and `gh`
//! are stubs on `PATH`, driven by files in a state directory.
#![cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const TARGETS: [&str; 6] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
];

const GIT: &str = r#"#!/bin/sh
[ -f "$STATE/tag_exists" ] || exit 2
if [ -n "$PEELED_SHA" ]; then
  printf '%s\trefs/tags/v1.2.3\n%s\trefs/tags/v1.2.3^{}\n' "$TAG_SHA" "$PEELED_SHA"
else
  printf '%s\trefs/tags/v1.2.3\n' "$TAG_SHA"
fi
"#;

const GH: &str = r#"#!/bin/sh
echo "$*" >> "$STATE/gh.log"
case "$1 $2" in
"release view")
  case "$(cat "$STATE/release")" in
    missing) echo "release not found" >&2; exit 1 ;;
    broken) echo "HTTP 502" >&2; exit 1 ;;
    draft) echo true ;;
    *) echo false ;;
  esac ;;
"release create")
  cp "$STAGED"/* "$REMOTE"/
  [ -z "$DROP" ] || rm -f "$REMOTE/$DROP"
  echo draft > "$STATE/release" ;;
"release upload") cp "$STAGED"/* "$REMOTE"/ ;;
"release edit") echo published > "$STATE/release"; touch "$STATE/tag_exists" ;;
"release download") cp "$REMOTE"/* "$5"/ ;;
"attestation verify") exit "${ATTEST_RC:-0}" ;;
*) echo "unexpected gh operation: $*" >&2; exit 1 ;;
esac
"#;

struct Harness {
    tmp: PathBuf,
}

impl Harness {
    /// `release` is the stub's release state: missing, broken, draft, published.
    fn new(label: &str, release: &str, tag_exists: bool) -> Harness {
        let tmp = std::env::temp_dir().join(format!("mind-release-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for dir in ["bin", "state", "remote", "staged", "resources"] {
            std::fs::create_dir_all(tmp.join(dir)).unwrap();
        }
        for (name, body) in [("git", GIT), ("gh", GH)] {
            let path = tmp.join("bin").join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(
            tmp.join("resources/changelog-section.sh"),
            "echo notes for \"$1\"\n",
        )
        .unwrap();
        std::fs::write(tmp.join("state/release"), release).unwrap();
        if tag_exists {
            std::fs::write(tmp.join("state/tag_exists"), "").unwrap();
        }
        for target in TARGETS {
            let name = format!("mind-1.2.3-{target}.tar.gz");
            std::fs::write(tmp.join("staged").join(name), format!("rebuilt {target}")).unwrap();
        }
        Harness { tmp }
    }

    /// Put the published form of every archive (and a matching SHA256SUMS) on
    /// the stub remote, with bytes that differ from the staged rebuild.
    fn publish(&self, only: usize) {
        for target in &TARGETS[..only] {
            let name = format!("mind-1.2.3-{target}.tar.gz");
            std::fs::write(
                self.tmp.join("remote").join(name),
                format!("published {target}"),
            )
            .unwrap();
        }
        if only == TARGETS.len() {
            let status = Command::new("bash")
                .arg("-c")
                .arg("sha256sum mind-*.tar.gz > SHA256SUMS")
                .current_dir(self.tmp.join("remote"))
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    fn run(&self, out: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new("bash");
        // Run from the harness directory, whose `resources/changelog-section.sh`
        // is a stub: the real one needs a CHANGELOG section for the version.
        cmd.current_dir(&self.tmp)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/publish-release.sh"
            ))
            .arg(self.tmp.join("staged"))
            .arg(self.tmp.join(out))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.tmp.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("VERSION", "1.2.3")
            .env("GITHUB_SHA", "abc")
            .env("TAG_SHA", "abc")
            .env("PEELED_SHA", "")
            .env("DROP", "")
            .env("GITHUB_REPOSITORY", "example/mind")
            .env("GH_TOKEN", "fixture")
            .env("STATE", self.tmp.join("state"))
            .env("REMOTE", self.tmp.join("remote"))
            .env("STAGED", self.tmp.join("staged"))
            .env("RUNNER_TEMP", &self.tmp);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.tmp.join("state/gh.log")).unwrap_or_default()
    }

    fn read(&self, dir: &str, target: &str) -> Vec<u8> {
        std::fs::read(
            self.tmp
                .join(dir)
                .join(format!("mind-1.2.3-{target}.tar.gz")),
        )
        .unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_ok(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
}

fn assert_no_write(h: &Harness) {
    let log = h.log();
    for op in ["release create", "release upload", "release edit"] {
        assert!(!log.contains(op), "{op} must not run: {log}");
    }
}

#[test]
// spec: STO-91
fn a_published_release_is_reused_and_never_changed() {
    let h = Harness::new("published", "published", true);
    h.publish(6);
    assert_ok(&h.run("out1", &[]));
    // A rerun after a downstream failure reuses the published bytes even though
    // the current build produced different archives.
    assert_ok(&h.run("out2", &[]));
    for target in TARGETS {
        assert_eq!(h.read("out1", target), h.read("out2", target));
        assert_eq!(h.read("remote", target), h.read("out2", target));
        assert_ne!(h.read("staged", target), h.read("out2", target));
    }
    assert_no_write(&h);
    assert!(h.log().contains("attestation verify"), "{}", h.log());
}

#[test]
// spec: STO-91
fn a_published_release_is_refused_when_the_tag_or_assets_disagree() {
    let h = Harness::new("published-bad", "published", true);
    h.publish(6);
    let wrong_tag = h.run("out", &[("TAG_SHA", "def")]);
    assert!(!wrong_tag.status.success());
    assert!(
        stderr(&wrong_tag).contains("points to def"),
        "{}",
        stderr(&wrong_tag)
    );
    // An annotated tag is judged by the commit it peels to.
    assert_ok(&h.run("out", &[("TAG_SHA", "tagobject"), ("PEELED_SHA", "abc")]));
    assert!(
        !h.run("out", &[("TAG_SHA", "abc"), ("PEELED_SHA", "def")])
            .status
            .success()
    );
    std::fs::write(h.tmp.join("remote/SHA256SUMS"), "incorrect\n").unwrap();
    assert!(!h.run("out", &[]).status.success());
    assert_no_write(&h);
}

#[test]
// spec: STO-91
fn a_published_release_without_provenance_is_refused() {
    let h = Harness::new("no-provenance", "published", true);
    h.publish(6);
    let out = h.run("out", &[("ATTEST_RC", "1")]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no valid build provenance"),
        "{}",
        stderr(&out)
    );
}

#[test]
// spec: STO-91
fn a_new_release_is_created_as_a_draft_verified_and_then_published() {
    let h = Harness::new("create", "missing", false);
    assert_ok(&h.run("out", &[]));
    let log = h.log();
    let create = log.find("release create").expect("create ran");
    let edit = log.find("release edit").expect("publish ran");
    assert!(create < edit, "{log}");
    let create_line = log
        .lines()
        .find(|l| l.starts_with("release create"))
        .unwrap();
    assert!(create_line.contains("--draft"), "{create_line}");
    assert!(create_line.contains("--target abc"), "{create_line}");
    assert!(log.contains("release edit v1.2.3 --draft=false"), "{log}");
    for target in TARGETS {
        assert_eq!(h.read("staged", target), h.read("out", target));
    }
}

#[test]
// spec: STO-91
fn a_draft_with_missing_assets_is_never_published() {
    let h = Harness::new("short-upload", "missing", false);
    let out = h.run("out", &[("DROP", "mind-1.2.3-x86_64-apple-darwin.tar.gz")]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("inconsistent asset set"),
        "{}",
        stderr(&out)
    );
    assert!(!h.log().contains("release edit"), "{}", h.log());
}

#[test]
// spec: STO-91
fn a_draft_left_by_a_failed_run_is_re_uploaded_and_published() {
    let h = Harness::new("draft", "draft", false);
    h.publish(2);
    assert_ok(&h.run("out", &[]));
    let log = h.log();
    assert!(log.contains("release upload v1.2.3"), "{log}");
    assert!(log.contains("--clobber"), "{log}");
    assert!(!log.contains("release create"), "{log}");
    assert!(log.contains("release edit v1.2.3 --draft=false"), "{log}");
    for target in TARGETS {
        assert_eq!(h.read("staged", target), h.read("out", target));
    }
}

#[test]
// spec: STO-91
fn a_release_lookup_failure_stops_before_any_write() {
    let h = Harness::new("lookup", "broken", false);
    let out = h.run("out", &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("HTTP 502"), "{}", stderr(&out));
    assert_no_write(&h);
}

#[test]
// spec: STO-91
fn a_published_release_whose_tag_is_missing_is_refused() {
    let h = Harness::new("no-tag", "published", false);
    h.publish(6);
    let out = h.run("out", &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("its tag is missing"),
        "{}",
        stderr(&out)
    );
    assert_no_write(&h);
}
