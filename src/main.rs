mod catalog;
mod cli;
mod commands;
mod config;
mod deps;
mod dump;
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
mod render;
mod resolve;
mod review;
mod selfupdate;
mod source;
mod tui;
mod unmanaged;

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
fn lock_mode(command: &Command, json: bool) -> LockMode {
    match command {
        // No persisted state touched (init-source operates on the repo dir, not
        // the store).
        Command::Completions { .. } | Command::Man | Command::InitSource { .. } => LockMode::None,

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
        Command::Probe { no_tui, .. } if probe_launches_tui(*no_tui, json) => LockMode::None,

        // Read-only commands (including probe in fallback/listing mode).
        Command::Recall { .. }
        | Command::Probe { .. }
        | Command::Review { .. }
        | Command::Introspect { fix: false, .. }
        | Command::Dump { .. }
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
    // Install the process-wide output context before any dispatch so that
    // render::ctx() returns real capabilities for all commands (including the
    // mutating verbs that read it internally). spec: CLI-150 CLI-151 CLI-154
    crate::render::set_ctx(crate::render::OutputCtx::detect(cli.json, cli.ascii));

    let paths = Paths::resolve()?;

    // spec: STO-40 STO-41 STO-42
    // Completions and man touch no persisted state: skip the lock. All other
    // commands acquire the lock (shared or exclusive) before reading or writing.
    match lock_mode(&cli.command, cli.json) {
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
    // Global flags sourced before the match moves cli.command.
    let json = cli.json;
    let yes = cli.yes;
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
            recursive,
            force,
        } => {
            // CLI-25: no repo argument (or an explicit `.`/`./`) melds the
            // current directory. Resolve it to an absolute path so `parse_spec`
            // derives a sensible `local/<parent>/<dir>` identity.
            let repo = match repo.as_deref() {
                None | Some(".") | Some("./") => std::env::current_dir()
                    .map_err(|e| crate::error::MindError::io(".", e))?
                    .to_string_lossy()
                    .into_owned(),
                Some(r) => r.to_string(),
            };
            // CLI-34: `--force` overwrites a conflicting target; otherwise a
            // conflict prompts on a TTY (Clobber::Prompt).
            let clobber = if force {
                commands::Clobber::Force
            } else {
                commands::Clobber::Prompt
            };
            let flow = commands::InstallFlow {
                yes,
                clobber,
                dangerously_skip: dangerously_skip_install_hook_check,
            };
            // CLI-12: re-melding an already-melded source is not an error; it
            // ensures the items are installed, else reports their status.
            if commands::is_melded(paths, &repo)? {
                commands::remeld(paths, &repo, alias, link_only, flow, recursive)?;
            } else {
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
                // CLI-23: by default, offer to install the melded source's items
                // right away (preview + prompt). `--link-only` stops at registering.
                if !link_only {
                    commands::install_melded_source(paths, &repo, flow)?;
                    // DSC-54 installs only the top-level source by default. Walk the
                    // curated chain and install each nested source the curator
                    // flagged `install = true` (DSC-58), or every nested source with
                    // `--recursive` (DSC-55).
                    if let Ok(top) = crate::source::parse_spec(&repo) {
                        commands::install_curated_sources(paths, &top.name, recursive, flow)?;
                    }
                }
            }
            // DSC-56: suggest `mind probe` after melding a curated super-source.
            commands::maybe_probe_hint(paths, &repo)
        }
        Command::InitSource { path, template } => commands::init_source(path.as_deref(), template),
        Command::Unmeld {
            name,
            unlink_only,
            uninstall_hook,
            dangerously_skip_install_hook_check,
        } => commands::unmeld(
            paths,
            &name,
            unlink_only,
            yes,
            dangerously_skip_install_hook_check,
            uninstall_hook,
        ),
        Command::Learn {
            item,
            all,
            dry_run,
            force,
            dangerously_skip_install_hook_check,
        } => {
            // CLI-36: `--all` rewrites the ref into the `<source>#*` selector.
            let item = if all {
                resolve::all_selector(&item)?
            } else {
                item
            };
            commands::learn(
                paths,
                &item,
                dry_run,
                commands::InstallFlow {
                    yes,
                    clobber: if force {
                        commands::Clobber::Force
                    } else {
                        commands::Clobber::Prompt
                    },
                    dangerously_skip: dangerously_skip_install_hook_check,
                },
            )
        }
        Command::Forget {
            item,
            unmanaged,
            force,
            dangerously_skip_install_hook_check,
        } => commands::forget(
            paths,
            item.as_deref(),
            unmanaged,
            yes,
            force,
            dangerously_skip_install_hook_check,
        ),
        Command::Sync {
            upgrade,
            dangerously_skip_install_hook_check,
        } => commands::sync(paths, upgrade, dangerously_skip_install_hook_check),
        Command::Upgrade {
            item,
            dangerously_skip_install_hook_check,
        } => commands::upgrade(
            paths,
            yes,
            item.as_deref(),
            dangerously_skip_install_hook_check,
        ),
        Command::Evolve { check, version } => selfupdate::run(check, yes, version),
        Command::Recall {
            sources,
            item,
            kind,
            source,
            tree,
        } => commands::recall(
            paths,
            sources,
            item.as_deref(),
            kind.map(|k| k.to_kind()),
            source.as_deref(),
            json,
            tree,
        ),
        Command::Probe {
            query,
            kind,
            source,
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
            fix,
        } => {
            if let Some(p) = policy {
                review::dispatch_policy(&p)
            } else {
                // CLI-26: no <target> (or an explicit `.`/`./`) reviews the
                // current directory, resolved to an absolute path so a local
                // source is identified.
                let target = match target.as_deref() {
                    None | Some(".") | Some("./") => std::env::current_dir()
                        .map_err(|e| crate::error::MindError::io(".", e))?
                        .to_string_lossy()
                        .into_owned(),
                    Some(t) => t.to_string(),
                };
                commands::review(paths, &target, alias, fix)
            }
        }
        Command::Introspect { fix } => commands::introspect(paths, fix, json),
        Command::Config { action } => match action {
            ConfigCmd::Show => commands::config_show(paths),
            ConfigCmd::Lobes { action } => match action {
                LobesCmd::Add { path } => commands::lobe_add(paths, &path),
                LobesCmd::List => commands::lobe_list(paths),
                LobesCmd::Remove { path } => commands::lobe_remove(paths, &path),
            },
        },
        Command::Dump {
            output,
            whole_sources,
        } => dump::run(paths, output, whole_sources),
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
        lock_mode(&cli.command, cli.json)
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
        // dump is read-only: registry + manifest + catalog only.
        // spec: DUMP-1
        assert_eq!(mode_of(&["mind", "dump"]), LockMode::Shared);
        assert_eq!(
            mode_of(&["mind", "dump", "--whole-sources"]),
            LockMode::Shared
        );
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

    /// `review` with no `<target>` and no `--policy` is a valid invocation: it
    /// defaults to the current directory (CLI-26). Supplying BOTH a `<target>`
    /// and `--policy` is a usage error (CLI-134). Verified at the parse layer.
    // spec: CLI-26 CLI-134
    #[test]
    fn review_target_and_policy_are_mutually_exclusive() {
        // Bare `review` parses (it defaults to `.` at dispatch time).
        assert!(
            Cli::try_parse_from(["mind", "review"]).is_ok(),
            "a bare `review` must parse (defaults to the current directory)"
        );
        // Both a target and --policy together is rejected by clap.
        assert!(
            Cli::try_parse_from(["mind", "review", "owner/repo", "--policy", "/tmp/p.toml"])
                .is_err(),
            "review with both a <target> and --policy must be a usage error"
        );
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
        assert!(!cli.yes, "global --yes should default to false");
        match cli.command {
            Command::Evolve { check, version } => {
                assert!(!check);
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

    /// Global --json suppresses the TUI for `probe` regardless of flag position.
    ///
    /// `mind probe --json` and `mind --json probe` must both be accepted by clap
    /// (global flag, CLI-150) and both cause `probe` to take the Shared lock
    /// (listing mode, not TUI mode).
    // spec: CLI-150
    #[test]
    fn global_json_suppresses_probe_tui_and_takes_shared_lock() {
        // Post-verb position: `mind probe --json`
        assert_eq!(
            mode_of(&["mind", "probe", "--json"]),
            LockMode::Shared,
            "probe --json (post-verb) must take the Shared lock (suppresses TUI)"
        );
        // Pre-verb position: `mind --json probe`
        assert_eq!(
            mode_of(&["mind", "--json", "probe"]),
            LockMode::Shared,
            "mind --json probe (pre-verb) must take the Shared lock (suppresses TUI)"
        );
        // Both positions parse identically at the Cli level.
        let post = Cli::try_parse_from(["mind", "probe", "--json"]).expect("probe --json parses");
        let pre = Cli::try_parse_from(["mind", "--json", "probe"]).expect("--json probe parses");
        assert!(post.json, "probe --json: cli.json must be true");
        assert!(pre.json, "--json probe: cli.json must be true");
    }

    /// Global --yes is accepted before or after any verb (CLI-150).
    // spec: CLI-150
    #[test]
    fn global_yes_is_accepted_before_or_after_verb() {
        // Post-verb: `mind learn --yes skill:foo`
        let cli = Cli::try_parse_from(["mind", "learn", "--yes", "skill:foo"])
            .expect("learn --yes should parse");
        assert!(cli.yes, "learn --yes: cli.yes must be true");

        // Pre-verb: `mind --yes learn skill:foo`
        let cli = Cli::try_parse_from(["mind", "--yes", "learn", "skill:foo"])
            .expect("--yes learn should parse");
        assert!(cli.yes, "--yes learn: cli.yes must be true");

        // Short form -y: `mind learn -y skill:foo`
        let cli = Cli::try_parse_from(["mind", "learn", "-y", "skill:foo"])
            .expect("learn -y should parse");
        assert!(cli.yes, "learn -y: cli.yes must be true");
    }

    /// Global --ascii is accepted before or after any verb (CLI-150).
    // spec: CLI-150
    #[test]
    fn global_ascii_is_accepted_before_or_after_verb() {
        // Post-verb: `mind probe --ascii`
        let cli =
            Cli::try_parse_from(["mind", "probe", "--ascii"]).expect("probe --ascii should parse");
        assert!(cli.ascii, "probe --ascii: cli.ascii must be true");

        // Pre-verb: `mind --ascii probe`
        let cli =
            Cli::try_parse_from(["mind", "--ascii", "probe"]).expect("--ascii probe should parse");
        assert!(cli.ascii, "--ascii probe: cli.ascii must be true");
    }
}
