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

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::resolve()?;

    // spec: STO-40 STO-41 STO-42
    // Completions and man touch no persisted state: skip the lock.
    // All other commands acquire the lock before reading or writing state.
    match &cli.command {
        Command::Completions { .. } | Command::Man => {
            return dispatch(cli, &paths);
        }
        _ => {}
    }

    let mut lock = lock::open(&paths)?;

    // Mutating commands need an exclusive lock; read-only commands need a shared
    // lock. Introspect --fix and Config lobes add/remove are mutating.
    let exclusive = matches!(
        &cli.command,
        Command::Meld { .. }
            | Command::Unmeld { .. }
            | Command::Learn { .. }
            | Command::Forget { .. }
            | Command::Sync { .. }
            | Command::Evolve { .. }
            | Command::Introspect { fix: true, .. }
            | Command::Config {
                action: ConfigCmd::Lobes {
                    action: LobesCmd::Add { .. } | LobesCmd::Remove { .. },
                },
            }
    );

    if exclusive {
        let _guard = lock.write()?;
        dispatch(cli, &paths)
    } else {
        let _guard = lock.read()?;
        dispatch(cli, &paths)
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
