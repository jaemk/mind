//! `mind review` dispatch and its JSON document.

use super::*;

/// `mind review <target> [--as <prefix>]` — validate a source for publishing.
///
/// Read-only. Collects hard errors and advisory findings; hard errors cause a
/// non-zero exit (CLI-132). Installs nothing and changes nothing on disk.
///
/// spec: CLI-130, CLI-131, CLI-132, CLI-133
pub fn review(paths: &Paths, target: &str, alias: Option<String>, fix: bool) -> Result<()> {
    let result = crate::review::review(paths, target, alias, fix)?;

    // Print hard then advisory findings in the shared format. Unconditional
    // even under `--json`: CLI-217's fd redirect routes it to stderr instead
    // of dropping it, exactly like every other verb's advisory output.
    crate::review::print_findings(&result.hard, &result.advisory);

    let out = crate::render::ctx();
    // Report any files `--fix` rewrote.
    for f in &result.fixed {
        println!("{} fixed {f}", out.ok());
    }

    if json_mode() {
        return emit_review_json(&result);
    }

    if result.hard.is_empty() {
        if result.advisory.is_empty() {
            println!("{} review: no issues found", out.ok());
        } else {
            println!(
                "{} review: {} advisory finding(s); source is publishable",
                out.warn(),
                result.advisory.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\n{} review: {} hard error(s), {} advisory finding(s)",
            out.err(),
            result.hard.len(),
            result.advisory.len()
        );
        Err(crate::error::MindError::ReviewFailed {
            hard: result.hard.len(),
        })
    }
}

/// `mind review --policy <path>` — validate a managed policy file, dispatching
/// to the JSON or text implementation.
///
/// Both `review::dispatch_policy` (text) and `review::review_policy` (JSON, via
/// this wrapper) are exercised from here, so neither `review.rs` function goes
/// unused in a bin crate (clippy `-D warnings` flags an orphaned `pub fn`).
///
/// spec: CLI-219 POL-50
pub fn review_policy_dispatch(path: &std::path::Path) -> Result<()> {
    if !json_mode() {
        return crate::review::dispatch_policy(path);
    }
    let result = crate::review::review_policy(path)?;
    emit_review_json(&result)
}

/// One `review` finding, JSON shape (CLI-219): the machine-stable `kind` tag
/// and the human message, exactly what the text mode prints as
/// `error [kind]: message` / `advisory [kind]: message`.
#[derive(Serialize)]
struct ReviewFindingJson<'a> {
    pub(super) kind: &'a str,
    pub(super) message: &'a str,
}

/// `mind review --json`'s result document (CLI-219).
#[derive(Serialize)]
struct ReviewJson<'a> {
    pub(super) schema: u8,
    pub(super) action: &'static str,
    pub(super) outcome: &'static str,
    pub(super) hard: Vec<ReviewFindingJson<'a>>,
    pub(super) advisory: Vec<ReviewFindingJson<'a>>,
    pub(super) fixed: &'a [String],
}

fn review_json_document(result: &crate::review::ReviewResult) -> ReviewJson<'_> {
    let outcome = if !result.hard.is_empty() {
        "failed"
    } else if !result.advisory.is_empty() {
        "advisory"
    } else {
        "clean"
    };
    fn finding_json(f: &crate::review::Finding) -> ReviewFindingJson<'_> {
        ReviewFindingJson {
            kind: f.kind,
            message: &f.message,
        }
    }
    ReviewJson {
        schema: 1,
        action: "review",
        outcome,
        hard: result.hard.iter().map(finding_json).collect(),
        advisory: result.advisory.iter().map(finding_json).collect(),
        fixed: &result.fixed,
    }
}

/// Emit `mind review --json`'s result document: `outcome: "clean"` when there
/// are no findings at all, `"advisory"` when only advisory findings exist.
/// Hard findings still fail `review` regardless of `--json` (CLI-132); the
/// document (with `outcome: "failed"`) is recorded as the CLI-181 error
/// envelope's `details` member (CLI-221) instead of printed as a success
/// envelope, so a machine caller sees exactly what failed on the non-zero
/// exit.
///
/// spec: CLI-219 CLI-221
fn emit_review_json(result: &crate::review::ReviewResult) -> Result<()> {
    let doc = review_json_document(result);
    if !result.hard.is_empty() {
        crate::json_stdout::record_error_details(&doc);
        return Err(crate::error::MindError::ReviewFailed {
            hard: result.hard.len(),
        });
    }
    print_json(&doc)
}
