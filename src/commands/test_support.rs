//! Fixtures shared by the `commands` unit tests.

use super::*;

pub(super) use crate::error::ItemKind;
pub(super) use crate::manifest::InstalledItem;
// The env lock is crate-wide, not per-module: `std::env::set_var` soundness
// is a process-wide property, and these tests share a binary with the
// env-mutating tests in `paths.rs` and `selfupdate.rs`.
pub(super) use crate::paths::ENV_LOCK;
pub(super) use std::path::PathBuf;
pub(super) use std::sync::atomic::{AtomicU32, Ordering};

pub(super) static COUNTER: AtomicU32 = AtomicU32::new(0);

pub(super) fn link_skill(name: &str, requires: Vec<String>) -> CatalogItem {
    CatalogItem {
        kind: ItemKind::Skill,
        name: name.to_string(),
        source: "local/test/repo#skills/review".to_string(),
        prefix: None,
        path: PathBuf::from("/nonexistent"),
        description: None,
        when_to_use: None,
        link_rel: None,
        bin: None,
        build: None,
        requires,
        expand: Vec::new(),
        hooks: Vec::new(),
        ignore: None,
    }
}

pub(super) fn catalog_skill(name: &str, source: &str, prefix: Option<&str>) -> CatalogItem {
    CatalogItem {
        prefix: prefix.map(str::to_string),
        source: source.to_string(),
        ..link_skill(name, vec![])
    }
}

/// A [`LearnTarget`] for readable assertions.
pub(super) fn target(source: &str, key: &str) -> LearnTarget {
    LearnTarget {
        source: source.to_string(),
        key: key.to_string(),
    }
}

pub(super) fn scratch(tag: &str) -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mind-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `policy_toml` to a temp file and point `$MIND_POLICY_FILE` at it,
/// also clearing `$MIND_AGENT_HOMES` so the outer env never bleeds in.
/// Returns the held env guard (drop last) and the base temp dir.
pub(super) fn with_policy(policy_toml: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-cmd-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let policy_file = base.join("policy.toml");
    std::fs::write(&policy_file, policy_toml).unwrap();
    // SAFETY: ENV_LOCK is held, so no other test reads env concurrently.
    unsafe {
        std::env::remove_var("MIND_AGENT_HOMES");
        std::env::set_var("MIND_POLICY_FILE", policy_file.to_str().unwrap());
    }
    (guard, base)
}

pub(super) fn item(name: &str, source: &str) -> InstalledItem {
    InstalledItem {
        kind: ItemKind::Skill,
        name: name.to_string(),
        bare_name: name.to_string(),
        source: source.to_string(),
        commit: "deadbeef".to_string(),
        hash: "h".to_string(),
        store: format!("store/skills/{name}"),
        links: vec![],
        description: None,
        install_hooks: Vec::new(),
        dropped_requires: Vec::new(),
        profiles: Vec::new(),
        explicit: false,
        version: None,
        lobe: None,
    }
}

// ---- F2: POL-40 override-note suppression --------------------------------

/// Build a bare `Source` for the `hook_rerun_warranted` truth table. Uses
/// `parse_spec` so the identity fields are filled the same way `meld` fills
/// them; the test only manipulates the commit and install_hooks fields.
///
/// `hooks` is a list of `(command, ran_at)` pairs to populate
/// `Source.install_hooks`.
pub(super) fn hook_source(
    commit: Option<&str>,
    hooks: &[(&str, Option<&str>)],
) -> crate::source::Source {
    use crate::source::RecordedSourceHook;
    let mut s = crate::source::parse_spec("acme/tools").expect("spec parses");
    s.commit = commit.map(str::to_string);
    s.install_hooks = hooks
        .iter()
        .map(|(cmd, ran_at)| RecordedSourceHook::install(*cmd, ran_at.map(str::to_string)))
        .collect();
    s
}

/// A throwaway `Paths` under the system temp dir, unique per call.
pub(super) fn ns_paths() -> (Paths, PathBuf) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-ns-set-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let paths = Paths::new(base.join("mind"), base.join("claude"));
    (paths, base)
}
