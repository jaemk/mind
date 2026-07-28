//! `--force`'s `--help` text must name both of its effects: overwriting a
//! colliding target in snapshot mode, and overriding the managed backfill's
//! `ensure_unoccupied` guard (HARN-17). Regression test so the two doc
//! comments in src/cli.rs cannot silently drift back to naming only one.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-help-force-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Sandbox {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    fn help(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn link_project_help_names_both_force_effects() {
    // spec: HARN-17 - --force overwrites a colliding snapshot target and also
    // overrides the backfill's ensure_unoccupied guard; --help must name both.
    let sb = Sandbox::new();
    let out = sb.help(&["link-project", "--help"]);
    assert!(
        out.contains("snapshot"),
        "link-project --help must still mention snapshot mode for --force: {out}"
    );
    assert!(
        out.contains("backfill"),
        "link-project --help must mention the backfill guard override for --force: {out}"
    );
}

#[test]
fn config_lobes_add_help_names_both_force_effects() {
    // spec: HARN-17 - same requirement for `config lobes add --force`.
    let sb = Sandbox::new();
    let out = sb.help(&["config", "lobes", "add", "--help"]);
    assert!(
        out.contains("snapshot"),
        "config lobes add --help must still mention snapshot mode for --force: {out}"
    );
    assert!(
        out.contains("backfill"),
        "config lobes add --help must mention the backfill guard override for --force: {out}"
    );
}
