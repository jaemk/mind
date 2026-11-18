//! Shared helpers for isolated CLI integration fixtures.
//!
//! Each test crate includes this with `mod common;` and uses only some of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

pub mod src_scan;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Reserve a per-process path for a fixture. The owning sandbox removes it on drop.
pub fn fixture_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    base
}

pub fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}
