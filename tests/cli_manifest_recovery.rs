//! Recovery of independently committed per-home manifest publications.
// spec: STO-87

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn fixture() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mind-manifest-recovery-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(path.join("default")).unwrap();
    fs::create_dir_all(path.join("extra")).unwrap();
    fs::create_dir_all(path.join("claude")).unwrap();
    path
}

fn run(base: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--json", "recall"])
        .env("MIND_HOME", base.join("default"))
        .env("MIND_HOMES", base.join("extra"))
        .env("CLAUDE_HOME", base.join("claude"))
        .output()
        .unwrap()
}

fn intent(destination: &Path, contents: &[u8]) -> serde_json::Value {
    serde_json::json!({"Manifest": {
        "staged": destination,
        "destination": destination,
        "contents": contents,
    }})
}

// spec: STO-87
#[test]
fn read_command_recovers_commits_in_each_configured_home() {
    let base = fixture();
    let first = base.join("default/manifest.json");
    let second = base.join("extra/manifest.json");
    let journal = base.join("default/.manifest-journal.json");
    let payload = br#"{"version":1,"items":{}}"#;
    fs::write(&first, payload).unwrap();
    fs::write(&second, b"old").unwrap();
    let first_doc = serde_json::json!({"version": 1, "decision": "Commit", "intents": [intent(&first, payload)]});
    let second_doc = serde_json::json!({"version": 1, "decision": "Commit", "intents": [intent(&second, payload)]});
    fs::write(&journal, serde_json::to_vec(&first_doc).unwrap()).unwrap();
    fs::write(
        base.join("extra/.manifest-journal.json"),
        serde_json::to_vec(&second_doc).unwrap(),
    )
    .unwrap();
    let output = run(&base);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read(first).unwrap(), payload);
    assert_eq!(fs::read(second).unwrap(), payload);
    assert!(!journal.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn recovery_follows_each_home_when_home_set_changes() {
    let base = fixture();
    let first = base.join("default/manifest.json");
    let second = base.join("extra/manifest.json");
    let payload = br#"{"version":1,"items":{}}"#;
    let first_journal = base.join("default/.manifest-journal.json");
    let second_journal = base.join("extra/.manifest-journal.json");
    for (destination, journal) in [(&first, &first_journal), (&second, &second_journal)] {
        let doc = serde_json::json!({"version":1,"decision":"Commit","intents":[intent(destination,payload)]});
        fs::write(journal, serde_json::to_vec(&doc).unwrap()).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--json", "recall"])
        .env("MIND_HOME", base.join("extra"))
        .env_remove("MIND_HOMES")
        .env("CLAUDE_HOME", base.join("claude"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read(second).unwrap(), payload);
    assert!(!second_journal.exists());
    assert!(
        first_journal.exists(),
        "omitted home's journal remains for its next use"
    );
    assert!(!first.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn recovery_does_not_invent_uncommitted_later_home_update() {
    let base = fixture();
    let first = base.join("default/manifest.json");
    let second = base.join("extra/manifest.json");
    let first_journal = base.join("default/.manifest-journal.json");
    let old = br#"{"version":1,"items":{}}"#;
    let new = br#"{"version":1,"items":{},"changed":true}"#;
    fs::write(&first, old).unwrap();
    fs::write(&second, old).unwrap();
    let document =
        serde_json::json!({"version":1,"decision":"Commit","intents":[intent(&first,new)]});
    fs::write(first_journal, serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(run(&base).status.success());
    assert_eq!(fs::read(first).unwrap(), new);
    assert_eq!(
        fs::read(second).unwrap(),
        old,
        "later home has no committed journal"
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn extra_home_journal_cannot_write_default_home() {
    let base = fixture();
    let victim = base.join("default/manifest.json");
    let journal = base.join("extra/.manifest-journal.json");
    let old = br#"{"version":1,"items":{}}"#;
    let payload = br#"{"version":1,"items":{},"changed":true}"#;
    fs::write(&victim, old).unwrap();
    let document = serde_json::json!({"version": 1, "decision": "Commit", "intents": [intent(&victim, payload)]});
    fs::write(&journal, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = run(&base);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"manifest-journal\""));
    assert_eq!(fs::read(victim).unwrap(), old);
    assert!(journal.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn interrupted_preparation_preserves_the_previous_manifest() {
    let base = fixture();
    let destination = base.join("default/manifest.json");
    let journal = base.join("default/.manifest-journal.json");
    let old = br#"{"version":1,"items":{}}"#;
    let replacement = br#"{"version":1,"items":{},"ignored":true}"#;
    fs::write(&destination, old).unwrap();
    let document = serde_json::json!({"version": 1, "decision": "Preparing", "intents": [intent(&destination, replacement)]});
    fs::write(&journal, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = run(&base);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read(destination).unwrap(), old);
    assert!(!journal.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn journal_cannot_overwrite_outside_allowed_manifest_paths() {
    let base = fixture();
    let victim = base.join("victim");
    let journal = base.join("default/.manifest-journal.json");
    fs::write(&victim, b"keep").unwrap();
    let payload = br#"{"version":1,"items":{}}"#;
    let document = serde_json::json!({"version": 1, "decision": "Commit", "intents": [intent(&victim, payload)]});
    fs::write(&journal, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = run(&base);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"manifest-journal\""));
    assert_eq!(fs::read(victim).unwrap(), b"keep");
    assert!(journal.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn dry_run_refuses_pending_recovery_without_modifying_state() {
    let base = fixture();
    let destination = base.join("default/manifest.json");
    let journal = base.join("default/.manifest-journal.json");
    let payload = br#"{"version":1,"items":{}}"#;
    let document = serde_json::json!({"version": 1, "decision": "Commit", "intents": [intent(&destination, payload)]});
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(&journal, &bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--json", "upgrade", "--dry-run"])
        .env("MIND_HOME", base.join("default"))
        .env("MIND_HOMES", base.join("extra"))
        .env("CLAUDE_HOME", base.join("claude"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"recovery-pending\""));
    assert_eq!(fs::read(&journal).unwrap(), bytes);
    assert!(!destination.exists());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn dangling_journal_symlink_fails_instead_of_retrying_forever() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};
    let base = fixture();
    let journal = base.join("default/.manifest-journal.json");
    symlink(base.join("missing"), &journal).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--json", "recall"])
        .env("MIND_HOME", base.join("default"))
        .env("MIND_HOMES", base.join("extra"))
        .env("CLAUDE_HOME", base.join("claude"))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("recall did not stop on a dangling journal");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    assert!(
        fs::symlink_metadata(journal)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn fifo_journal_is_rejected_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};
    let base = fixture();
    let journal = base.join("default/.manifest-journal.json");
    let path = CString::new(journal.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--json", "recall"])
        .env("MIND_HOME", base.join("default"))
        .env("MIND_HOMES", base.join("extra"))
        .env("CLAUDE_HOME", base.join("claude"))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("recall blocked on a FIFO journal");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn recovery_publishes_manifest_with_normal_creation_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let base = fixture();
    let destination = base.join("default/manifest.json");
    let reference = base.join("default/reference");
    fs::write(&destination, b"old").unwrap();
    fs::write(&reference, b"reference").unwrap();
    let expected = fs::metadata(&reference).unwrap().permissions().mode() & 0o777;
    let payload = br#"{"version":1,"items":{}}"#;
    let document = serde_json::json!({"version":1,"decision":"Commit","intents":[intent(&destination,payload)]});
    fs::write(
        base.join("default/.manifest-journal.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    assert!(run(&base).status.success());
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        expected
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn oversized_journal_is_rejected_and_retained() {
    let base = fixture();
    let journal = base.join("default/.manifest-journal.json");
    let f = fs::File::create(&journal).unwrap();
    f.set_len(16 * 1024 * 1024 + 1).unwrap();
    let output = run(&base);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"manifest-journal\""), "{stdout}");
    assert!(stdout.contains("size limit"), "{stdout}");
    assert!(journal.exists());
    fs::remove_dir_all(base).unwrap();
}
