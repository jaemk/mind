//! Thin wrapper around the `git` CLI, surfacing failures as [`MindError::Git`].

use std::path::Path;
use std::process::Command;

use crate::error::{MindError, Result};

/// Run `git <args>` in `cwd`, returning trimmed stdout on success.
fn run(url: &str, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(MindError::GitNotFound),
        Err(e) => {
            return Err(MindError::Git {
                url: url.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                status: None,
                stderr: e.to_string(),
            });
        }
    };

    if !output.status.success() {
        return Err(MindError::Git {
            url: url.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            status: Some(output.status),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clone `url` into `dest` (shallow).
pub fn clone(url: &str, dest: &Path) -> Result<()> {
    let dest = dest.to_string_lossy().into_owned();
    run(url, None, &["clone", "--depth", "1", url, &dest])?;
    Ok(())
}

/// Fetch and hard-reset an existing clone to its remote default branch.
pub fn fetch_and_reset(url: &str, dir: &Path) -> Result<()> {
    run(url, Some(dir), &["fetch", "--depth", "1", "origin"])?;
    // Reset to whatever origin's HEAD points at.
    let head = run(url, Some(dir), &["rev-parse", "origin/HEAD"]).or_else(|_| {
        // Some remotes don't advertise origin/HEAD after a shallow fetch.
        run(url, Some(dir), &["rev-parse", "FETCH_HEAD"])
    })?;
    run(url, Some(dir), &["reset", "--hard", &head])?;
    Ok(())
}

/// Resolve the current HEAD commit sha of a clone.
pub fn head_commit(url: &str, dir: &Path) -> Result<String> {
    run(url, Some(dir), &["rev-parse", "HEAD"])
}
