mod catalog;
mod cli;
mod commands;
mod config;
mod error;
mod frontmatter;
mod git;
mod hash;
mod install;
mod lock;
mod manifest;
mod mindfile;
mod namespace;
mod paths;
mod resolve;
mod source;

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
        | Command::Evolve { .. }
        | Command::Introspect { fix: true, .. }
        | Command::Config {
            action:
                ConfigCmd::Lobes {
                    action: LobesCmd::Add { .. } | LobesCmd::Remove { .. },
                },
        } => LockMode::Exclusive,

        // Read-only commands.
        Command::Recall { .. }
        | Command::Probe { .. }
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
        Command::Meld { repo, alias } => commands::meld(paths, &repo, alias),
        Command::Unmeld { name, forget } => commands::unmeld(paths, &name, forget),
        Command::Learn { item, dry_run } => commands::learn(paths, &item, dry_run),
        Command::Forget { item } => commands::forget(paths, &item),
        Command::Sync { evolve } => commands::sync(paths, evolve),
        Command::Evolve { yes, item } => commands::evolve(paths, yes, item.as_deref()),
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
        } => commands::probe(
            paths,
            query.as_deref(),
            kind.map(|k| k.to_kind()),
            source.as_deref(),
            json,
        ),
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
        assert_eq!(mode_of(&["mind", "meld", "owner/repo"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "unmeld", "src"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "learn", "review"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "forget", "review"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "sync"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "sync", "--evolve"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "evolve"]), LockMode::Exclusive);
        assert_eq!(mode_of(&["mind", "evolve", "--yes"]), LockMode::Exclusive);
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
}
