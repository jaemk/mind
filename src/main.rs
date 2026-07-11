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
mod hooks_cmd;
mod install;
mod lock;
mod manifest;
mod mindfile;
mod namespace;
mod paths;
mod plugin_manifest;
mod policy;
mod render;
mod resolve;
mod review;
mod sanitize;
mod scaffold;
mod selfupdate;
mod source;
mod tui;
mod unmanaged;

use std::io::IsTerminal;

use clap::Parser;

use cli::{Cli, Command, ConfigCmd, HooksCmd, LobesCmd};
use error::Result;
use paths::Paths;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    // Capture the json flag before `cli` is moved into `run`. Clap's parse
    // succeeded at this point, so cli.json is trustworthy.
    let json = cli.json;
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            if json {
                // spec: CLI-181 -- emit the error as a JSON envelope on stdout so
                // that scripts parsing stdout get a machine-readable reason. The
                // exit code is unchanged (FAILURE = 1). Plain-text stderr output
                // is suppressed; the envelope carries the full Display message.
                let envelope = serde_json::json!({
                    "schema": 1,
                    "error": {
                        "kind": err.kind(),
                        "message": err.to_string(),
                    }
                });
                // Ignore the Result: we are already in the error path.
                let _ = crate::render::print_json(&envelope);
            } else {
                // Structured errors print their own Display; print the source chain too.
                eprintln!("error: {err}");
                let mut src = std::error::Error::source(&err);
                while let Some(e) = src {
                    eprintln!("  caused by: {e}");
                    src = e.source();
                }
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

        // `evolve` takes NO outer command lock. It manages the binary swap under
        // its own exclusive lock inside `download_and_swap` (STO-46), acquired only
        // after the network-free decision/prompt phase. Classifying it Exclusive
        // here would take the same lock on a first fd and then deadlock when the
        // inner step blocks acquiring it on a second fd (flock contends across two
        // fds in the same process). `evolve --check` touches no state at all.
        // spec: STO-48
        Command::Evolve { .. } => LockMode::None,

        // Mutating commands.
        Command::Meld { .. }
        | Command::Unmeld { .. }
        | Command::Learn { .. }
        | Command::Forget { .. }
        | Command::Sync { .. }
        | Command::Upgrade { .. }
        | Command::Absorb { .. }
        | Command::Introspect { fix: true, .. }
        // hooks run mutates sources.json (recorded run-commits) and, for --event
        // build, the store; it needs the exclusive lock (spec: HOOK-101/HOOK-103).
        | Command::Hooks {
            action: HooksCmd::Run { .. },
        }
        | Command::Config {
            action:
                ConfigCmd::Lobes {
                    action: LobesCmd::Add { .. } | LobesCmd::Remove { .. } | LobesCmd::Detect,
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
        // hooks list is read-only: it reports but never runs or records anything.
        | Command::Hooks {
            action: HooksCmd::List { .. },
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
    crate::render::set_ctx(crate::render::OutputCtx::detect(
        cli.json,
        cli.ascii,
        cli.verbose,
    ));

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
            flat_skills,
            follow_branch,
            pin_tag,
            pin_ref,
            install_hook,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
            register_only,
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
                dangerously_skip_build: dangerously_skip_build_hook_check,
            };
            // CLI-12: re-melding an already-melded source is not an error; it
            // ensures the items are installed, else reports their status.
            if commands::is_melded(paths, &repo)? {
                commands::remeld(paths, &repo, alias, register_only, flow, recursive)?;
            } else {
                let meld_sum = commands::meld(
                    paths,
                    &repo,
                    alias,
                    roots,
                    flat_skills,
                    follow_branch,
                    pin_tag,
                    pin_ref,
                    install_hook,
                    dangerously_skip_install_hook_check,
                )?;
                // CLI-23: by default, offer to install the melded source's items
                // right away (preview + prompt). `--register-only` stops at registering.
                //
                // CLI-156: in json mode the entire meld+install outcome is folded
                // into ONE JSON object emitted here. Human output is unchanged.
                if !register_only {
                    if json {
                        // Install silently (no separate JSON from learn), collect keys.
                        let (mut inst, pend) = commands::install_source_items_for_json(
                            paths,
                            &meld_sum.source_name,
                            flow,
                        )?;
                        // Also walk the curated chain silently (DSC-54/55/58).
                        if let Ok(top) = crate::source::parse_spec(&repo) {
                            let curated = commands::install_curated_sources_for_json(
                                paths, &top.name, recursive, flow,
                            )?;
                            inst.extend(curated);
                        }
                        commands::emit_meld_json_result(meld_sum, inst, pend)?;
                    } else {
                        commands::install_melded_source(paths, &repo, flow)?;
                        // DSC-54 installs only the top-level source by default. Walk the
                        // curated chain and install each nested source the curator
                        // flagged `install = true` (DSC-58), or every nested source with
                        // `--recursive` (DSC-55).
                        if let Ok(top) = crate::source::parse_spec(&repo) {
                            commands::install_curated_sources(paths, &top.name, recursive, flow)?;
                        }
                    }
                } else if json {
                    // register-only + json: register only, emit the meld result now.
                    commands::emit_meld_json_result(meld_sum, vec![], 0)?;
                }
            }
            // DSC-56: suggest `mind probe` after melding a curated super-source.
            commands::maybe_probe_hint(paths, &repo)
        }
        Command::InitSource {
            path,
            template,
            marketplace,
            flat_skills,
            namespace,
        } => commands::init_source(
            path.as_deref(),
            template,
            marketplace,
            flat_skills,
            namespace,
        ),
        Command::Unmeld {
            name,
            keep_items,
            uninstall_hook,
            dangerously_skip_install_hook_check,
        } => commands::unmeld(
            paths,
            &name,
            keep_items,
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
            dangerously_skip_build_hook_check,
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
                    dangerously_skip_build: dangerously_skip_build_hook_check,
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
            dangerously_skip_build_hook_check,
        } => commands::sync(
            paths,
            upgrade,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
        ),
        Command::Upgrade {
            item,
            no_sync,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
        } => {
            // spec: CLI-169 - default syncs first; --no-sync skips the fetch.
            if no_sync {
                commands::upgrade_no_sync(
                    paths,
                    yes,
                    item.as_deref(),
                    dangerously_skip_install_hook_check,
                    dangerously_skip_build_hook_check,
                )
            } else {
                commands::upgrade(
                    paths,
                    yes,
                    item.as_deref(),
                    dangerously_skip_install_hook_check,
                    dangerously_skip_build_hook_check,
                )
            }
        }
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
                LobesCmd::Add { path, preset } => match (path, preset) {
                    (None, Some(name)) => commands::lobe_add_preset(paths, &name, yes),
                    (Some(p), None) => commands::lobe_add(paths, &p, yes),
                    // clap's `conflicts_with` rejects supplying both.
                    (Some(_), Some(_)) => unreachable!("path and --preset conflict"),
                    (None, None) => Err(crate::error::MindError::LobeTargetRequired),
                },
                LobesCmd::List => commands::lobe_list(paths),
                LobesCmd::Detect => commands::lobe_detect(paths, yes),
                LobesCmd::Remove { path } => commands::lobe_remove(paths, &path),
            },
        },
        Command::Absorb {
            item_ref,
            to,
            force,
        } => commands::absorb(paths, &item_ref, to, force, yes),
        Command::Hooks { action } => match action {
            HooksCmd::Run {
                target,
                event,
                force,
                dangerously_skip_install_hook_check,
                dangerously_skip_build_hook_check,
            } => hooks_cmd::run(
                paths,
                &target,
                event,
                force,
                dangerously_skip_install_hook_check,
                dangerously_skip_build_hook_check,
            ),
            HooksCmd::List { target } => hooks_cmd::list(paths, &target),
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
        // detect may add lobes and mutates config when --yes / global -y is set.
        assert_eq!(
            mode_of(&["mind", "config", "lobes", "detect"]),
            LockMode::Exclusive,
            "config lobes detect must take the exclusive lock"
        );
        // Global -y and --yes are both accepted for detect (CLI-150).
        assert_eq!(
            mode_of(&["mind", "-y", "config", "lobes", "detect"]),
            LockMode::Exclusive,
            "mind -y config lobes detect must parse and take the exclusive lock"
        );
        assert_eq!(
            mode_of(&["mind", "--yes", "config", "lobes", "detect"]),
            LockMode::Exclusive,
            "mind --yes config lobes detect must parse and take the exclusive lock"
        );
        // absorb mutates the manifest, the lobe, and optionally the config.
        assert_eq!(
            mode_of(&["mind", "absorb", "skill:review"]),
            LockMode::Exclusive,
            "absorb mutates state and must take the exclusive lock"
        );
        assert_eq!(
            mode_of(&["mind", "absorb", "skill:review", "--to", "/tmp/dest"]),
            LockMode::Exclusive
        );
        assert_eq!(
            mode_of(&["mind", "absorb", "--force", "agent:dev"]),
            LockMode::Exclusive
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

    /// `hooks run` takes the exclusive lock (it mutates sources.json recorded
    /// run-commits and, for --event build, the store). `hooks list` is read-only
    /// and takes the shared lock.
    // spec: HOOK-100 HOOK-101 HOOK-103 CLI-194 CLI-195 CLI-196
    #[test]
    fn hooks_run_exclusive_list_shared() {
        assert_eq!(
            mode_of(&["mind", "hooks", "run", "agents"]),
            LockMode::Exclusive,
            "hooks run must take the exclusive lock"
        );
        assert_eq!(
            mode_of(&["mind", "hooks", "run", "agents", "--event", "install"]),
            LockMode::Exclusive,
        );
        assert_eq!(
            mode_of(&[
                "mind",
                "hooks",
                "run",
                "agents#skill:scan",
                "--event",
                "build"
            ]),
            LockMode::Exclusive,
        );
        assert_eq!(
            mode_of(&["mind", "hooks", "list", "agents"]),
            LockMode::Shared,
            "hooks list must take the shared lock"
        );
    }

    /// `hooks run --event` accepts install, uninstall, build value variants and
    /// parses correctly. Force, dangerously-skip flags parse as expected.
    // spec: CLI-195
    #[test]
    fn hooks_run_flags_parse() {
        use cli::HookEventArg;
        // Default event is install.
        let cli = Cli::try_parse_from(["mind", "hooks", "run", "agents"])
            .expect("bare hooks run should parse");
        match cli.command {
            Command::Hooks {
                action:
                    HooksCmd::Run {
                        target,
                        event,
                        force,
                        dangerously_skip_install_hook_check,
                        dangerously_skip_build_hook_check,
                    },
            } => {
                assert_eq!(target, "agents");
                assert_eq!(event, HookEventArg::Install);
                assert!(!force);
                assert!(!dangerously_skip_install_hook_check);
                assert!(!dangerously_skip_build_hook_check);
            }
            other => panic!("expected HooksCmd::Run, got {other:?}"),
        }

        // --event build parses.
        let cli = Cli::try_parse_from([
            "mind",
            "hooks",
            "run",
            "agents#skill:scan",
            "--event",
            "build",
            "--force",
            "--dangerously-skip-install-hook-check",
            "--dangerously-skip-build-hook-check",
        ])
        .expect("hooks run with all flags should parse");
        match cli.command {
            Command::Hooks {
                action:
                    HooksCmd::Run {
                        event,
                        force,
                        dangerously_skip_install_hook_check,
                        dangerously_skip_build_hook_check,
                        ..
                    },
            } => {
                assert_eq!(event, HookEventArg::Build);
                assert!(force);
                assert!(dangerously_skip_install_hook_check);
                assert!(dangerously_skip_build_hook_check);
            }
            other => panic!("expected HooksCmd::Run, got {other:?}"),
        }

        // hooks list parses.
        let cli = Cli::try_parse_from(["mind", "hooks", "list", "owner/repo"])
            .expect("hooks list should parse");
        match cli.command {
            Command::Hooks {
                action: HooksCmd::List { target },
            } => assert_eq!(target, "owner/repo"),
            other => panic!("expected HooksCmd::List, got {other:?}"),
        }
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
                ..
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
                ..
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
    /// and classifies `None` (no outer command lock): it acquires the exclusive
    /// lock itself inside `download_and_swap` (STO-46), so an outer exclusive lock
    /// would deadlock the inner acquisition (C4). `--version` resolves offline.
    // spec: STO-48
    #[test]
    fn evolve_self_update_parses_and_takes_no_outer_lock() {
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

        // All three forms classify None: `evolve` takes the exclusive lock itself
        // inside download_and_swap (STO-46). Taking it here too would deadlock the
        // inner acquisition on a second fd (C4 regression guard).
        assert_eq!(mode_of(&["mind", "evolve"]), LockMode::None);
        assert_eq!(mode_of(&["mind", "evolve", "--check"]), LockMode::None);
        assert_eq!(
            mode_of(&["mind", "evolve", "--version", "1.2.3"]),
            LockMode::None
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

    /// `config lobes detect` reads confirmation from the global -y/--yes (CLI-150).
    ///
    /// The local `yes` field was removed from `LobesCmd::Detect`; the global
    /// `cli.yes` is the sole source of the flag for this subcommand.
    // spec: CLI-150
    #[test]
    fn detect_uses_global_yes_not_local() {
        // Post-verb --yes: `mind config lobes detect --yes`
        let cli = Cli::try_parse_from(["mind", "config", "lobes", "detect", "--yes"])
            .expect("detect --yes should parse");
        assert!(cli.yes, "detect --yes: cli.yes must be true");
        assert!(
            matches!(
                cli.command,
                Command::Config {
                    action: ConfigCmd::Lobes {
                        action: LobesCmd::Detect
                    }
                }
            ),
            "command must be Detect unit variant"
        );

        // Pre-verb --yes: `mind --yes config lobes detect`
        let cli = Cli::try_parse_from(["mind", "--yes", "config", "lobes", "detect"])
            .expect("--yes detect should parse");
        assert!(cli.yes, "--yes detect: cli.yes must be true");

        // Short form -y post-verb: `mind config lobes detect -y`
        let cli = Cli::try_parse_from(["mind", "config", "lobes", "detect", "-y"])
            .expect("detect -y should parse");
        assert!(cli.yes, "detect -y: cli.yes must be true");

        // Short form -y pre-verb: `mind -y config lobes detect`
        let cli = Cli::try_parse_from(["mind", "-y", "config", "lobes", "detect"])
            .expect("-y detect should parse");
        assert!(cli.yes, "-y detect: cli.yes must be true");
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

    /// Global --verbose/-v is accepted before or after any verb and defaults false (CLI-162).
    // spec: CLI-162
    #[test]
    fn global_verbose_is_accepted_before_or_after_verb() {
        // Default: cli.verbose is false.
        let cli = Cli::try_parse_from(["mind", "probe"]).expect("probe should parse");
        assert!(!cli.verbose, "cli.verbose must default to false");

        // Post-verb long form: `mind probe --verbose`
        let cli = Cli::try_parse_from(["mind", "probe", "--verbose"])
            .expect("probe --verbose should parse");
        assert!(cli.verbose, "probe --verbose: cli.verbose must be true");

        // Pre-verb long form: `mind --verbose probe`
        let cli = Cli::try_parse_from(["mind", "--verbose", "probe"])
            .expect("--verbose probe should parse");
        assert!(cli.verbose, "--verbose probe: cli.verbose must be true");

        // Post-verb short form: `mind probe -v`
        let cli = Cli::try_parse_from(["mind", "probe", "-v"]).expect("probe -v should parse");
        assert!(cli.verbose, "probe -v: cli.verbose must be true");

        // Pre-verb short form: `mind -v probe`
        let cli = Cli::try_parse_from(["mind", "-v", "probe"]).expect("-v probe should parse");
        assert!(cli.verbose, "-v probe: cli.verbose must be true");
    }
}
