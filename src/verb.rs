//! What `main` needs to know about a parsed [`Command`] before running it:
//! the lock it takes, whether `--json` reserves stdout, whether it is a dry
//! run, the verb name its hook records carry, and the items hooks it settles
//! afterwards. Every per-verb classification lives here so a new verb is
//! classified in one file.

use std::io::IsTerminal;

use crate::cli::{AutoUpdateCmd, Command, ConfigCmd, HooksCmd, LobesCmd};
use crate::items_hook;

/// The lock a command must hold before it touches persisted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockMode {
    /// No lock: the command touches no persisted state (`completions`, `man`).
    None,
    /// Shared lock: a read-only command (multiple readers may proceed at once).
    Shared,
    /// Exclusive lock: a mutating command (excludes all other holders).
    Exclusive,
}

/// Decide which lock a command needs. This is the single source of truth for the
/// STO-41 mapping; it is unit-tested per variant so a new or reclassified command
/// cannot silently take the wrong lock.
///
/// For `probe` in interactive TUI mode (TTY + no opt-out), no outer lock is
/// acquired: the TUI takes the lock per-operation itself (TUI-25). In fallback
/// mode (non-TTY, `--no-tui`, `--json`), `probe` takes the normal shared lock.
// spec: STO-41
pub(crate) fn lock_mode(command: &Command, json: bool, ascii: bool) -> LockMode {
    match command {
        // No persisted state touched (init-source operates on the repo dir, not
        // the store).
        Command::Completions { .. } | Command::Man | Command::InitSource { .. } => LockMode::None,

        // `evolve` takes NO outer command lock. It manages the binary swap under
        // its own exclusive lock inside `download_and_swap` (STO-46), acquired only
        // after the network-free decision/prompt phase. Classifying it Exclusive
        // here would take the same lock on a first fd and then deadlock when the
        // inner step blocks acquiring it on a second fd (flock contends across two
        // fds in the same process). `evolve --check` touches no state at all.
        // spec: STO-48
        Command::Evolve { .. } => LockMode::None,

        // `auto-update run` takes the exclusive lock itself, without waiting
        // (AUP-18): an outer blocking lock would queue an unattended run behind
        // the user, and `--background` takes no lock at all in the parent.
        // spec: AUP-18
        Command::AutoUpdate {
            action: AutoUpdateCmd::Run { .. },
        } => LockMode::None,

        Command::Upgrade { dry_run: true, .. }
        | Command::Unmeld { dry_run: true, .. }
        | Command::Forget { dry_run: true, .. }
        | Command::Prune { dry_run: true, .. } => LockMode::Shared,

        // Mutating commands.
        Command::Meld(_)
        // curate registers, installs, re-pins, upgrades, and (with --prune)
        // unmelds: the same exclusive-lock class as the verbs it applies.
        | Command::Curate { .. }
        | Command::Unmeld { .. }
        | Command::Learn { .. }
        | Command::Forget { .. }
        | Command::Prune { .. }
        | Command::Sync { .. }
        | Command::Upgrade { .. }
        | Command::Absorb { .. }
        | Command::Introspect { fix: true, .. }
        // link-project mutates config and symlinks (or writes snapshot files).
        // spec: CLI-198
        | Command::LinkProject { .. }
        // hooks run mutates sources.json (recorded run-commits) and, for --event
        // build, the store; it needs the exclusive lock (spec: HOOK-101/HOOK-103).
        | Command::Hooks {
            action: HooksCmd::Run { .. },
        }
        | Command::Config {
            action:
                ConfigCmd::Lobes {
                    action: LobesCmd::Add { .. } | LobesCmd::Remove { .. } | LobesCmd::Detect,
                }
                | ConfigCmd::AutoUpdate { .. },
        } => LockMode::Exclusive,

        // probe in TUI mode: the TUI manages its own per-op locks (TUI-25).
        // TUI-1 is the launch entry point (requires a real TTY; allowlisted).
        // spec: TUI-25
        Command::Probe { no_tui, .. } if probe_launches_tui(*no_tui, json, ascii) => LockMode::None,

        // Read-only commands (including probe in fallback/listing mode).
        Command::Recall { .. }
        | Command::Probe { .. }
        | Command::Review { .. }
        | Command::Introspect { fix: false, .. }
        | Command::Dump { .. }
        // hooks list and hooks history are read-only: they report but never
        // run or record anything.
        | Command::Hooks {
            action: HooksCmd::List { .. } | HooksCmd::History { .. },
        }
        | Command::Config {
            action:
                ConfigCmd::Show
                | ConfigCmd::Lobes {
                    action: LobesCmd::List,
                },
        } => LockMode::Shared,
    }
}

/// Whether `--json` makes stdout the exclusive channel for this verb's one
/// result document (CLI-217), so it may be reserved for the whole run.
///
/// The default is to reserve: a verb added later is protected without anyone
/// having to remember this function. Only a verb whose stdout payload is NOT a
/// JSON document needs an entry below, and getting that wrong is loud (its
/// output disappears from stdout in the first test that pipes it). CLI-218
/// states this exclusion list as the closed boundary; every verb not listed
/// here answers `--json` with exactly one document, `review` and `hooks list`
/// included (CLI-219, CLI-220) -- neither is excluded any more.
// spec: CLI-217 CLI-218
pub(crate) fn json_reserves_stdout(command: &Command) -> bool {
    !matches!(
        command,
        // stdout carries a non-JSON product, `--json` or not:
        //   dump      -- DUMP-9: TOML on stdout, plus a stderr note that --json
        //                does not apply
        //   completions/man -- the script / roff page IS the output
        Command::Dump { .. } | Command::Completions { .. } | Command::Man
        // `evolve` writes its own document straight to stdout from
        // `selfupdate.rs` rather than through `commands/`, so redirecting fd 1
        // would silence it.
        | Command::Evolve { .. }
        // A maintainer scaffolder that edits the target repo in place; it has
        // no JSON result to offer, so it prints human text on stdout in every
        // mode.
        | Command::InitSource { .. }
    )
}

/// True when `probe` will launch the interactive TUI: the flags permit it AND
/// stdout is a TTY. This is the single test for the TUI/fallback branch; it is
/// used in both `lock_mode` and `dispatch` so the decision stays consistent.
///
/// `probe` falls back to the plain (`--no-tui`) listing not only on `--no-tui`,
/// `--json`, and a non-TTY stdout (TUI-2), but also when `--ascii` is given or
/// the active locale is not UTF-8 (TUI-71): the TUI draws Unicode box glyphs, so
/// a Unicode-hostile output mode must not silently launch it.
///
/// TUI-1 (interactive launch) requires a real TTY to verify; it is allowlisted
/// rather than cited. TUI-2/TUI-71 (fallback) are tested in tests/cli/.
// spec: TUI-2 TUI-71
pub(crate) fn probe_launches_tui(no_tui: bool, json: bool, ascii: bool) -> bool {
    !no_tui && !json && !ascii && utf8_locale() && std::io::stdout().is_terminal()
}

/// Whether the active locale advertises UTF-8 (TUI-71). Checks `LC_ALL`,
/// `LC_CTYPE`, `LANG` in that order (first set wins); returns `false` when none
/// is set (a conservative ASCII default). Mirrors `render::detect_utf8_locale`,
/// which is private to that module; kept here so the probe launch gate does not
/// depend on the color/glyph capability context (which also folds in `NO_COLOR`,
/// a signal that must NOT block the TUI -- TUI-65 renders it monochrome).
pub(crate) fn utf8_locale() -> bool {
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Some(val) = std::env::var_os(var) {
            let lower = val.to_string_lossy().to_lowercase();
            if lower.is_empty() {
                continue;
            }
            return lower.contains("utf-8") || lower.contains("utf8");
        }
    }
    false
}

/// Whether the command only previews (`--dry-run`): it then refuses to run
/// over an unrecovered transaction rather than recovering it (STO-87, LIFE-61).
pub(crate) fn is_dry_run(command: &Command) -> bool {
    matches!(
        command,
        Command::Upgrade { dry_run: true, .. }
            | Command::Unmeld { dry_run: true, .. }
            | Command::Forget { dry_run: true, .. }
            | Command::Prune { dry_run: true, .. }
            | Command::Learn { dry_run: true, .. }
    )
}

/// The verb a hook record names (HOOK-160).
pub(crate) fn verb_name(command: &Command) -> &'static str {
    match command {
        Command::Meld(_) => "meld",
        Command::Unmeld { .. } => "unmeld",
        Command::Learn { .. } => "learn",
        Command::Forget { .. } => "forget",
        Command::Prune { .. } => "prune",
        Command::Sync { .. } => "sync",
        Command::Upgrade { .. } => "upgrade",
        Command::Curate { .. } => "curate",
        Command::Absorb { .. } => "absorb",
        Command::Probe { .. } => "probe",
        Command::Hooks {
            action: HooksCmd::Run { .. },
        } => "hooks run",
        Command::AutoUpdate { .. } => "auto-update",
        _ => "other",
    }
}

/// The verb name, settle scope, and accepted events for a command that can
/// change installed items (HOOK-191), or `None` for one that cannot. `unmeld`
/// offers its source's items hooks itself, before the source's uninstall
/// hooks (HOOK-192), and so is not listed.
pub(crate) fn items_settle(
    command: &Command,
) -> Option<(&'static str, items_hook::Scope, crate::hook::Accept)> {
    use items_hook::Scope;
    Some(match command {
        Command::Meld(args) => ("meld", Scope::Touched, args.accept.accept()),
        Command::Learn {
            dry_run: false,
            accept,
            ..
        } => ("learn", Scope::Touched, accept.accept()),
        Command::Forget { dry_run: true, .. } | Command::Prune { dry_run: true, .. } => {
            return None;
        }
        Command::Forget { accept, .. } => ("forget", Scope::Touched, accept.accept()),
        Command::Prune { accept, .. } => ("prune", Scope::Touched, accept.accept()),
        Command::Curate {
            check: false,
            accept,
            ..
        } => ("curate", Scope::Touched, accept.accept()),
        Command::Absorb { .. } => ("absorb", Scope::Touched, crate::hook::Accept::default()),
        // spec: HOOK-191 -- an upgrade also offers what an earlier run left
        // pending, in the sources it covers.
        Command::Sync {
            source,
            upgrade: true,
            accept,
            ..
        } => ("sync", Scope::Pending(source.clone()), accept.accept()),
        Command::Sync { accept, .. } => ("sync", Scope::Touched, accept.accept()),
        Command::Upgrade { dry_run: true, .. } => return None,
        Command::Upgrade {
            item: Some(_),
            source: None,
            accept,
            ..
        } => ("upgrade", Scope::Touched, accept.accept()),
        Command::Upgrade { source, accept, .. } => {
            ("upgrade", Scope::Pending(source.clone()), accept.accept())
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `probe_launches_tui` forces the plain listing for every Unicode-hostile
    /// output mode (TUI-71): `--no-tui`, `--json`, and `--ascii` each return
    /// `false` without needing a real TTY (the flag short-circuits ahead of the
    /// terminal check). The positive (real-TTY, UTF-8) launch is TUI-1, which
    /// needs a PTY and is allowlisted. `utf8_locale` reads the locale env vars in
    /// precedence order, treating an unset locale as non-UTF-8.
    // spec: TUI-71
    #[test]
    fn probe_gate_forces_fallback_for_unicode_hostile_modes() {
        assert!(
            !probe_launches_tui(true, false, false),
            "--no-tui must fall back to the listing"
        );
        assert!(
            !probe_launches_tui(false, true, false),
            "--json must fall back to the listing"
        );
        assert!(
            !probe_launches_tui(false, false, true),
            "--ascii must fall back to the listing (no Unicode glyphs)"
        );

        // Locale detection: serialize env mutation crate-wide, save/restore.
        let _g = crate::paths::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(&str, Option<std::ffi::OsString>)> = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .map(|k| (*k, std::env::var_os(k)))
            .collect();
        // SAFETY: ENV_LOCK is held, so no concurrent env access on other threads.
        unsafe {
            for (k, _) in &saved {
                std::env::remove_var(k);
            }
            assert!(
                !utf8_locale(),
                "no locale vars set => non-UTF-8 (conservative)"
            );

            std::env::set_var("LANG", "C");
            assert!(!utf8_locale(), "LANG=C is not UTF-8");

            std::env::set_var("LANG", "en_US.UTF-8");
            assert!(utf8_locale(), "LANG=en_US.UTF-8 is UTF-8");

            // LC_ALL wins over LANG when both are set.
            std::env::set_var("LC_ALL", "C");
            assert!(!utf8_locale(), "LC_ALL=C overrides a UTF-8 LANG");

            // Restore.
            for (k, v) in &saved {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}
