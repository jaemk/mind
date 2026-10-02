//! A malformed `config.toml` and the metadata cap (DSC-104): the config is only
//! the cap's origin when it names the cap key, so a malformed file that does not
//! name it must not fail verbs that never read config.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

struct Sandbox {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-badcfg-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("mind")).unwrap();
        Sandbox {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    fn run(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_POLICY_FILE")
            .env_remove("MIND_MAX_METADATA_SIZE")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn config(&self, body: &str) {
        write(&self.mind_home.join("config.toml"), body);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The cap resolution must not be what fails: a malformed config that does not
/// name the cap key is never reported as a config error by `hooks list` (which
/// reads no config), while the same file naming the key is.
// spec: DSC-104
#[test]
fn malformed_config_without_cap_key_does_not_fail_cap_resolution() {
    let sb = Sandbox::new();
    sb.config("this is = = not toml\n");
    let r = sb.run(&["hooks", "list", "nonexistent-target"]);
    assert!(
        !r.stderr.contains("config.toml"),
        "cap resolution must not surface the config error: {}{}",
        r.stdout,
        r.stderr
    );
}

// spec: DSC-104
#[test]
fn malformed_config_naming_the_cap_key_still_errors() {
    let sb = Sandbox::new();
    sb.config("max-metadata-size = = 1MiB\n");
    let r = sb.run(&["hooks", "list", "nonexistent-target"]);
    assert!(!r.success, "{}{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("config"), "{}", r.stderr);
}

// spec: DSC-104
#[test]
fn flag_outranks_a_malformed_config_naming_the_cap_key() {
    let sb = Sandbox::new();
    sb.config("max-metadata-size = = 1MiB\n");
    let r = sb.run(&[
        "--max-metadata-size",
        "1MiB",
        "hooks",
        "list",
        "nonexistent-target",
    ]);
    assert!(
        !r.stderr.contains("config.toml"),
        "{}{}",
        r.stdout,
        r.stderr
    );
}
