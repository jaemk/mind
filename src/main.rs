mod catalog;
mod cli;
mod commands;
mod config;
mod deps;
mod error;
mod frontmatter;
mod git;
mod hash;
mod hook;
mod install;
mod lock;
mod manifest;
mod mindfile;
mod namespace;
mod paths;
mod policy;
mod resolve;
mod review;
mod selfupdate;
mod source;
mod tui;

use std::io::IsTerminal;

use clap::Parser;

use cli::{Cli, Command, ConfigCmd, LobesCmd};
use error::Result;
use paths::Paths;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // Structured errors print their own Display; print the source chain too.
            eprintln!("error: {err}");
            let mut src = std::error::Error::source(&err);
            while let Some(e) = src {
                eprintln!("  caused by: {e}");
                src = e.source();
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// The lock a command must hold before it touches persisted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockMode {
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
fn lock_mode(command: &Command) -> LockMode {
    match command {
        // No persisted state touched.
        Command::Completions { .. } | Command::Man => LockMode::None,

        // Mutating commands.
        Command::Meld { .. }
        | Command::Unmeld { .. }
        | Command::Learn { .. }
        | Command::Forget { .. }
        | Command::Sync { .. }
        | Command::Upgrade { .. }
        | Command::Evolve { .. }
        | Command::Introspect { fix: true, .. }
        | Command::Config {
            action:
                ConfigCmd::Lobes {
                    action: LobesCmd::Add { .. } | LobesCmd::Remove { .. },
                },
        } => LockMode::Exclusive,

        // probe in TUI mode: the TUI manages its own per-op locks (TUI-25).
        // TUI-1 is the launch entry point (requires a real TTY; allowlisted).
        // spec: TUI-25
        Command::Probe { no_tui, json, .. } if probe_launches_tui(*no_tui, *json) => LockMode::None,

        // Read-only commands (including probe in fallback/listing mode).
        Command::Recall { .. }
        | Command::Probe { .. }
        | Command::Review { .. }
        | Command::Introspect { fix: false, .. }
        | Command::Config {
            action:
                ConfigCmd::Show
                | ConfigCmd::Lobes {
                    action: LobesCmd::List,
                },
        } => LockMode::Shared,
    }
}

/// True when `probe` will launch the interactive TUI: the flags permit it AND
/// stdout is a TTY. This is the single test for the TUI/fallback branch; it is
/// used in both `lock_mode` and `dispatch` so the decision stays consistent.
///
/// TUI-1 (interactive launch) requires a real TTY to verify; it is allowlisted
/// rather than cited. TUI-2 (fallback) is tested in tests/cli.rs.
// spec: TUI-2
fn probe_launches_tui(no_tui: bool, json: bool) -> bool {
    !no_tui && !json && std::io::stdout().is_terminal()
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::resolve()?;

    // spec: STO-40 STO-41 STO-42
    // Completions and man touch no persisted state: skip the lock. All other
    // commands acquire the lock (shared or exclusive) before reading or writing.
    match lock_mode(&cli.command) {
        LockMode::None => dispatch(cli, &paths),
        LockMode::Exclusive => {
            let mut lock = lock::open(&paths)?;
            let _guard = lock.write()?;
            dispatch(cli, &paths)
        }
        LockMode::Shared => {
            let lock = lock::open(&paths)?;
            let _guard = lock.read()?;
            dispatch(cli, &paths)
        }
    }
}

fn dispatch(cli: Cli, paths: &Paths) -> Result<()> {
    match cli.command {
        Command::Meld {
            repo,
            alias,
            roots,
            follow_branch,
            pin_tag,
            pin_ref,
            install_hook,
            dangerously_skip_install_hook_check,
            link_only,
            yes,
        } => {
            commands::meld(
                paths,
                &repo,
                alias,
                roots,
                follow_branch,
                pin_tag,
                pin_ref,
                install_hook,
                dangerously_skip_install_hook_check,
            )?;
            // CLI-23: by default, offer to install the melded source's items right
            // away (preview + prompt). `--link-only` stops at registering it.
            if link_only {
                Ok(())
            } else {
                commands::install_melded_source(paths, &repo, yes)
            }
        }
        Command::Unmeld { name, forget } => commands::unmeld(paths, &name, forget),
        Command::Learn { item, dry_run, yes } => commands::learn(paths, &item, dry_run, yes),
        Command::Forget { item } => commands::forget(paths, &item),
        Command::Sync {
            upgrade,
            dangerously_skip_install_hook_check,
        } => commands::sync(paths, upgrade, dangerously_skip_install_hook_check),
        Command::Upgrade {
            yes,
            item,
            dangerously_skip_install_hook_check,
        } => commands::upgrade(
            paths,
            yes,
            item.as_deref(),
            dangerously_skip_install_hook_check,
        ),
        Command::Evolve {
            check,
            yes,
            version,
        } => selfupdate::run(check, yes, version),
        Command::Recall {
            sources,
            item,
            kind,
            source,
            json,
        } => commands::recall(
            paths,
            sources,
            item.as_deref(),
            kind.map(|k| k.to_kind()),
            source.as_deref(),
            json,
        ),
        Command::Probe {
            query,
            kind,
            source,
            json,
            no_tui,
        } => {
            if probe_launches_tui(no_tui, json) {
                // TUI mode: the interactive browser manages its own locks.
                // spec: TUI-2
                tui::run(
                    paths,
                    query.as_deref(),
                    kind.map(|k| k.to_kind()),
                    source.as_deref(),
                )
            } else {
                // Fallback listing mode.
                // spec: TUI-2
                commands::probe(
                    paths,
                    query.as_deref(),
                    kind.map(|k| k.to_kind()),
                    source.as_deref(),
                    json,
                )
            }
        }
        Command::Review {
            target,
            alias,
            policy,
        } => match (target, policy) {
            (_, Some(p)) => review::dispatch_policy(&p),
            (Some(t), None) => commands::review(paths, &t, alias),
            (None, None) => {
                eprintln!(
                    "error: `review` requires either a <target> or --policy <path>; see `mind review --help`"
                );
                Err(crate::error::MindError::ReviewFailed { hard: 1 })
            }
        },
        Command::Introspect { fix, json } => commands::introspect(paths, fix, json),
        Command::Config { action } => match action {
            ConfigCmd::Show => commands::config_show(paths),
            ConfigCmd::Lobes { action } => match action {
                LobesCmd::Add { path } => commands::lobe_add(paths, &path),
                LobesCmd::List => commands::lobe_list(paths),
                LobesCmd::Remove { path } => commands::lobe_remove(paths, &path),
            },
        },
        Command::Completions { shell } => {
            commands::completions(shell);
            Ok(())
        }
        Command::Man => commands::man(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Parse a CLI line the way the binary would, then classify its lock mode.
    fn mode_of(args: &[&str]) -> LockMode {
        let cli = Cli::try_parse_from(args).expect("args should parse");
        lock_mode(&cli.command)
    }

    #[test]
    fn mutating_commands_take_the_exclusive_lock() {
        // Every mutating verb must hold the exclusive lock so its
        // read-modify-write cycle is never interleaved with another process.
        // spec: STO-41
        assert_eq!(
            mode_of(&["mind", "meld", "owner/repo"]),
            LockMode::Exclusive
        );
        assert_eq!(mode_of(&["mind", "unmeld", "src"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "learn", "review"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "forget", "review"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "sync"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "sync", "--upgrade"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "upgrade"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "upgrade", "--yes"]), LockMode::Exclusive);
        // `evolve` is now the binary self-update verb; it mutates the on-disk
        // binary and must take the exclusive lock.
        assert_eq!(mode_of(&["mind", "evolve"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "evolve", "--check"]), LockMode::Exclusive);
        assert_eq!(
            mode_of(&["mind", "evolve", "--version", "1.2.3"]),
            LockMode::Exclusive
        );
        // introspect --fix is mutating (it recreates links) and MUST be exclusive,
        // not shared. This is the easy-to-get-wrong case.
        assert_eq!(
            mode_of(&["mind", "introspect", "--fix"]),
            LockMode::Exclusive,
            "introspect --fix repairs state and must take the exclusive lock"
        );
        assert_eq!(
            mode_of(&["mind", "config", "lobes", "add", "/some/home"]),
            LockMode::Exclusive
        );
        assert_eq!(
            mode_of(&["mind", "config", "lobes", "remove", "/some/home"]),
            LockMode::Exclusive,
            "config lobes remove mutates config and must take the exclusive lock"
        );
    }

    #[test]
    fn read_only_commands_take_the_shared_lock() {
        // Read-only verbs take the shared lock so concurrent readers proceed but
        // never observe a writer mid-update.
        // spec: STO-41
        assert_eq!(mode_of(&["mind", "recall"]), LockMode::Shared);
        assert_eq!(mode_of(&["mind", "recall", "--sources"]), LockMode::Shared);
        assert_eq!(mode_of(&["mind", "probe"]), LockMode::Shared);
        assert_eq!(mode_of(&["mind", "probe", "rev"]), LockMode::Shared);
        // review is read-only: it installs nothing and changes no disk state.
        assert_eq!(mode_of(&["mind", "review", "/some/path"]), LockMode::Shared);
        assert_eq!(
            mode_of(&["mind", "review", "/some/path", "--as", "jk"]),
            LockMode::Shared
        );
        // review --policy is also read-only (POL-50).
        assert_eq!(
            mode_of(&["mind", "review", "--policy", "/etc/mind/policy.toml"]),
            LockMode::Shared
        );
        // introspect WITHOUT --fix is a read-only diagnosis -> shared.
        assert_eq!(mode_of(&["mind", "introspect"]), LockMode::Shared);
        assert_eq!(mode_of(&["mind", "config", "show"]), LockMode::Shared);
        assert_eq!(
            mode_of(&["mind", "config", "lobes", "list"]),
            LockMode::Shared
        );
    }

    #[test]
    fn lockless_commands_take_no_lock() {
        // completions and man touch no persisted state, so they skip the lock
        // entirely (and so work even with no mind home).
        // spec: STO-40 STO-41
        assert_eq!(mode_of(&["mind", "completions", "bash"]), LockMode::None);
        assert_eq!(mode_of(&["mind", "man"]), LockMode::None);
    }

    /// `mind review` with neither a `<target>` nor `--policy` is a usage error:
    /// the `(None, None)` dispatch arm prints guidance and returns
    /// `ReviewFailed { hard: 1 }` so the process exits non-zero. This arm is only
    /// reachable through `dispatch`, so drive it there directly.
    /// spec: POL-50
    #[test]
    fn review_without_target_or_policy_is_usage_error() {
        let cli = Cli::try_parse_from(["mind", "review"]).expect("bare review parses");
        // The arm returns before touching persisted state, so any Paths works.
        let paths = Paths {
            mind_home: std::env::temp_dir().join("mind-review-usage-test"),
            claude_home: std::env::temp_dir().join("mind-review-usage-test-claude"),
        };
        match dispatch(cli, &paths) {
            Err(error::MindError::ReviewFailed { hard }) => {
                assert_eq!(hard, 1, "no-target/no-policy is a single hard usage error");
            }
            other => panic!("expected Err(ReviewFailed) usage error, got {other:?}"),
        }
    }

    #[test]
    fn introspect_fix_flag_flips_the_lock_mode() {
        // Pin the exact boundary: the same verb is shared without --fix and
        // exclusive with it. A regression that ignored the flag would fail here.
        // spec: STO-41
        assert_ne!(
            mode_of(&["mind", "introspect"]),
            mode_of(&["mind", "introspect", "--fix"]),
            "introspect and introspect --fix must take different locks"
        );
    }

    /// `sync --upgrade --dangerously-skip-install-hook-check` must parse and the
    /// flag must be forwarded to the upgrade pass so non-TTY CI can trigger hook
    /// re-runs unattended (HOOK-11, HOOK-23). Verified here by inspecting the
    /// parsed struct; the end-to-end behavior is covered by tests/cli.rs.
    // spec: HOOK-11 HOOK-23
    #[test]
    fn sync_dangerously_skip_install_hook_check_parses() {
        // Without the flag: parses, field is false.
        let cli = Cli::try_parse_from(["mind", "sync", "--upgrade"])
            .expect("sync --upgrade should parse");
        match cli.command {
            Command::Sync {
                upgrade,
                dangerously_skip_install_hook_check,
            } => {
                assert!(upgrade, "--upgrade should be true");
                assert!(
                    !dangerously_skip_install_hook_check,
                    "flag absent: should be false"
                );
            }
            other => panic!("expected Sync, got {other:?}"),
        }

        // With the flag: parses, field is true.
        let cli = Cli::try_parse_from([
            "mind",
            "sync",
            "--upgrade",
            "--dangerously-skip-install-hook-check",
        ])
        .expect("sync --upgrade --dangerously-skip-install-hook-check should parse");
        match cli.command {
            Command::Sync {
                upgrade,
                dangerously_skip_install_hook_check,
            } => {
                assert!(upgrade, "--upgrade should be true");
                assert!(
                    dangerously_skip_install_hook_check,
                    "flag present: should be true"
                );
            }
            other => panic!("expected Sync, got {other:?}"),
        }

        // Flag without --upgrade is now a parse error (HOOK-23: the flag requires
        // --upgrade so it cannot be a silent no-op).
        assert!(
            Cli::try_parse_from(["mind", "sync", "--dangerously-skip-install-hook-check"]).is_err(),
            "sync --dangerously-skip-install-hook-check without --upgrade must be a parse error"
        );

        // Confirm the lock mode is still Exclusive with the new flag.
        assert_eq!(
            mode_of(&[
                "mind",
                "sync",
                "--upgrade",
                "--dangerously-skip-install-hook-check"
            ]),
            LockMode::Exclusive,
            "sync --upgrade --dangerously-skip-install-hook-check must take the exclusive lock"
        );
    }

    /// `evolve` (the binary self-update verb) parses with and without its flags
    /// and classifies Exclusive. `--version` resolves the target offline.
    #[test]
    fn evolve_self_update_parses_and_is_exclusive() {
        // Bare evolve parses.
        let cli = Cli::try_parse_from(["mind", "evolve"]).expect("evolve should parse");
        match cli.command {
            Command::Evolve {
                check,
                yes,
                version,
            } => {
                assert!(!check);
                assert!(!yes);
                assert_eq!(version, None);
            }
            other => panic!("expected Evolve, got {other:?}"),
        }

        // evolve --check parses with the flag set.
        let cli =
            Cli::try_parse_from(["mind", "evolve", "--check"]).expect("evolve --check parses");
        match cli.command {
            Command::Evolve { check, .. } => assert!(check, "--check should be true"),
            other => panic!("expected Evolve, got {other:?}"),
        }

        // evolve --version <v> carries the explicit version.
        let cli = Cli::try_parse_from(["mind", "evolve", "--version", "1.2.3"])
            .expect("evolve --version parses");
        match cli.command {
            Command::Evolve { version, .. } => {
                assert_eq!(version.as_deref(), Some("1.2.3"));
            }
            other => panic!("expected Evolve, got {other:?}"),
        }

        // All three forms classify Exclusive (they mutate the on-disk binary).
        assert_eq!(mode_of(&["mind", "evolve"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "evolve", "--check"]), LockMode::Exclusive);
        assert_eq!(
            mode_of(&["mind", "evolve", "--version", "1.2.3"]),
            LockMode::Exclusive
        );
    }
}
