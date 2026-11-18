//! Settings-read gate: production code reads the process environment, the home
//! directory, `config.toml`, and the managed policy file only through
//! `src/settings.rs` (`Env` and `Settings`), so each setting's name and
//! precedence live in one place. Test modules may read and set variables
//! directly (fixtures save and restore them).

mod common;

use std::path::PathBuf;

use common::src_scan::{files_with_ext, find_test_module};

/// Calls that read a setting without going through `settings.rs`.
const DIRECT_READS: &[&str] = &[
    "std::env::var(",
    "std::env::var_os(",
    "env::vars(",
    "dirs::home_dir(",
    "Config::load(",
    "Policy::load(",
];

#[test]
fn settings_are_read_only_through_the_settings_module() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for file in files_with_ext(&src, "rs") {
        if file.ends_with("settings.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let production = &text[..find_test_module(&text).unwrap_or(text.len())];
        for (n, line) in production.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if DIRECT_READS.iter().any(|call| line.contains(call)) {
                found.push(format!(
                    "{}:{}: {}",
                    file.strip_prefix(&src).unwrap().display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "read these through `settings::Env` or `Paths::settings()` instead:\n{}",
        found.join("\n")
    );
}
