//! Refresh source clones concurrently while preserving registry order.

use super::{clone_dir_checked, sync_clone_to_pin};
use crate::error::{MindError, Result};
use crate::git;
use crate::paths::Paths;
use crate::source::Pin;

/// Bring one source's clone up to date for `sync` / the `upgrade` pre-pass and
/// return its new HEAD with the pin to record. A linked (no-pin) local source
/// is its live working tree (CLI-27): nothing is fetched or touched, its HEAD
/// is only re-read (best effort).
fn refresh_source(paths: &Paths, source: &crate::source::Source) -> Result<(String, Pin)> {
    // spec: STO-69 -- validate before any fetch/clone/delete touches it.
    let dir = clone_dir_checked(paths, source)?;
    if source.is_linked() {
        // The working tree must still exist; a deleted one is a sync error
        // for this source (CLI-54 reports it and continues with the rest).
        if !dir.is_dir() {
            return Err(MindError::NotADirectory {
                path: dir.display().to_string(),
            });
        }
        let new_commit = git::head_commit(&source.url, &dir)
            .ok()
            .or_else(|| source.commit.clone())
            .unwrap_or_default();
        return Ok((new_commit, source.pin.clone()));
    }
    // CLI-55: resolve the source against its recorded pin (never change the
    // pin itself, only move HEAD to the pinned point). A version pin records
    // its new resolution (VER-43).
    sync_clone_to_pin(source, &dir)
}

/// The default for the most source fetches `refresh_sources` runs at once.
const DEFAULT_MAX_PARALLEL_FETCHES: usize = 8;

/// Parse a `MIND_MAX_PARALLEL_FETCHES` value: a positive integer, else the
/// default (missing, non-numeric, and zero all fall back).
// spec: CLI-249
pub(super) fn parse_max_parallel_fetches(raw: Option<&str>) -> usize {
    match raw.and_then(|v| v.trim().parse::<usize>().ok()) {
        None | Some(0) => DEFAULT_MAX_PARALLEL_FETCHES,
        Some(n) => n,
    }
}

/// The most source fetches `refresh_sources` runs at once
/// (`MIND_MAX_PARALLEL_FETCHES`, default 8).
fn max_parallel_fetches() -> usize {
    parse_max_parallel_fetches(crate::settings::Env.max_parallel_fetches().as_deref())
}

/// [`refresh_source`] for each of `sources`, fetching up to
/// [`max_parallel_fetches`] at once. Results come back in input order. A limit
/// of 1 is the one-at-a-time sync, prompts and all.
///
/// Each source's clone is its own directory, so the fetches are independent.
/// A worker's git children never prompt (concurrent prompts on one terminal
/// would interleave), so a source that failed where an interactive git would
/// have prompted (credentials, an ssh passphrase or host key) is retried
/// serially afterwards with prompting allowed, as a one-at-a-time sync would
/// have run it.
// spec: CLI-248
pub(super) fn refresh_sources(
    paths: &Paths,
    sources: &[&crate::source::Source],
) -> Vec<Result<(String, Pin)>> {
    let workers = sources.len().min(max_parallel_fetches());
    if workers <= 1 {
        return sources
            .iter()
            .map(|s| {
                // spec: PRG-4
                let _step = crate::progress::step(format!("syncing {}", s.name));
                refresh_source(paths, s)
            })
            .collect();
    }
    // spec: PRG-4 -- one counted step for the pool; the workers' git calls
    // run under it rather than replacing its label.
    let total = sources.len();
    let step = crate::progress::step(format!("syncing sources (0/{total})"));
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Vec<(usize, Result<(String, Pin)>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    git::with_noninteractive(|| {
                        let mut mine = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(source) = sources.get(i) else {
                                break mine;
                            };
                            mine.push((i, refresh_source(paths, source)));
                            let n = finished.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            step.set(format!("syncing sources ({}/{total})", n + 1));
                        }
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    let mut slots: Vec<Option<Result<(String, Pin)>>> = sources.iter().map(|_| None).collect();
    for (i, r) in done {
        slots[i] = Some(r);
    }
    drop(step);
    slots
        .into_iter()
        .zip(sources)
        .map(|(slot, source)| match slot {
            // spec: PRG-6 -- the retry is the prompting run, so it goes with
            // the spinner suspended and git prompts at once rather than
            // failing a batch attempt first.
            Some(Err(e)) if git::is_suppressed_prompt_failure(&e) && !git::is_noninteractive() => {
                crate::progress::suspend(|| refresh_source(paths, source))
            }
            Some(r) => r,
            // Every index below `sources.len()` is claimed by exactly one worker.
            None => unreachable!("refresh_sources: source {} was never fetched", source.name),
        })
        .collect()
}
