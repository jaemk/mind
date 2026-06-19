mod catalog;
mod cli;
mod commands;
mod config;
mod error;
mod frontmatter;
mod git;
mod hash;
mod install;
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
    match cli.command {
        Command::Meld { repo, alias } => commands::meld(&paths, &repo, alias),
        Command::Unmeld { name, forget } => commands::unmeld(&paths, &name, forget),
        Command::Learn { item, dry_run } => commands::learn(&paths, &item, dry_run),
        Command::Forget { item } => commands::forget(&paths, &item),
        Command::Sync { evolve } => commands::sync(&paths, evolve),
        Command::Evolve { yes, item } => commands::evolve(&paths, yes, item.as_deref()),
        Command::Recall {
            sources,
            item,
            kind,
            source,
        } => commands::recall(
            &paths,
            sources,
            item.as_deref(),
            kind.map(|k| k.to_kind()),
            source.as_deref(),
        ),
        Command::Probe {
            query,
            kind,
            source,
        } => commands::probe(
            &paths,
            query.as_deref(),
            kind.map(|k| k.to_kind()),
            source.as_deref(),
        ),
        Command::Introspect { fix } => commands::introspect(&paths, fix),
        Command::Config { action } => match action {
            ConfigCmd::Show => commands::config_show(&paths),
            ConfigCmd::Lobes { action } => match action {
                LobesCmd::Add { path } => commands::lobe_add(&paths, &path),
                LobesCmd::List => commands::lobe_list(&paths),
                LobesCmd::Remove { path } => commands::lobe_remove(&paths, &path),
            },
        },
    }
}
