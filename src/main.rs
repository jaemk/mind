mod auto_update;
mod catalog;
mod cli;
mod commands;
mod config;
mod consent;
mod curate;
mod deps;
mod dump;
mod error;
mod frontmatter;
mod git;
mod hash;
mod hook;
mod hook_log;
mod hooks_cmd;
mod ignore;
mod install;
mod items_hook;
mod json_stdout;
mod lock;
mod manifest;
mod mindfile;
mod namespace;
mod paths;
mod plugin_manifest;
mod policy;
mod profile;
mod progress;
mod prompt;
mod render;
mod resolve;
mod review;
mod sanitize;
mod scaffold;
mod selfupdate;
mod source;
mod transaction;
mod tui;
mod unmanaged;
mod verb;
mod version;
mod workflow_check;
mod workflow_meta;

use clap::{CommandFactory, Parser};

use cli::{AutoUpdateCmd, Cli, Command, ConfigCmd, HooksCmd, LobesCmd, OnOff};
use config::MAX_METADATA_SIZE_ENV;
use error::{MindError, Result};
use paths::Paths;
use verb::{
    LockMode, is_dry_run, items_settle, json_reserves_stdout, lock_mode, probe_launches_tui,
    verb_name,
};

fn main() -> std::process::ExitCode {
    // spec: CLI-207 -- bare `mind` prints help on stdout and exits 0.
    // `arg_required_else_help` alone is not enough: clap renders the resulting
    // `DisplayHelpOnMissingArgumentOrSubcommand` to stderr and exits 2, which
    // is the usage-error shape, not the "here is what this tool does" shape a
    // first-time user typing `mind` should get.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            print!("{}", Cli::command().render_help());
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => e.exit(),
    };
    // Capture the json flag before `cli` is moved into `run`. Clap's parse
    // succeeded at this point, so cli.json is trustworthy.
    let json = cli.json;
    match run(cli) {
        Ok(()) => {
            // spec: CLI-217 -- the single point stdout is written under `--json`.
            json_stdout::flush_pending();
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                // spec: CLI-181 -- emit the error as a JSON envelope on stdout so
                // that scripts parsing stdout get a machine-readable reason. The
                // exit code is the error's own, as in text mode (CLI-175).
                // Plain-text stderr output is suppressed; the envelope carries
                // the full Display message.
                // spec: CLI-221 -- folds in any findings the verb recorded
                // before failing (e.g. `commands::review`'s hard findings)
                // as the envelope's `details` member.
                let envelope = json_stdout::build_error_envelope(err.kind(), &err.to_string());
                // spec: CLI-217 -- the envelope REPLACES any result a verb
                // recorded before failing, so stdout is one document either way.
                json_stdout::emit_instead(&envelope);
            } else {
                // Structured errors print their own Display; print the source chain too.
                eprintln!("error: {err}");
                let mut src = std::error::Error::source(&err);
                while let Some(e) = src {
                    eprintln!("  caused by: {e}");
                    src = e.source();
                }
            }
            // spec: CLI-175 CLI-255 -- a hook failure or interruption has its
            // own code, so a script can tell it from other errors.
            std::process::ExitCode::from(err.exit_code())
        }
    }
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
    // spec: PRG-2
    crate::progress::init(cli.json, cli.no_progress, cli.ascii);

    // spec: CLI-217 -- reserve stdout BEFORE anything can print to it, so the
    // guarantee does not depend on where in the verb the first line is written.
    if cli.json && json_reserves_stdout(&cli.command) {
        json_stdout::reserve();
    }

    let paths = Paths::resolve()?;

    // spec: DSC-103 DSC-104 -- install the metadata cap before any dispatch, so
    // every metadata read in this run uses one ceiling.
    //
    // The config file is consulted only when neither the flag nor the
    // environment supplied a value, and a config that will not parse falls back
    // to the default rather than failing here: verbs that never read config
    // (`completions`, `man`) must not start failing on a malformed one, and the
    // verbs that do read it report the parse error themselves, with the context
    // this early in the run does not have. (`recall` is NOT in that list: it
    // reads config through `unmanaged::scan` -> `Paths::agent_homes` ->
    // `Config::load`.)
    //
    // An empty `MIND_MAX_METADATA_SIZE=` reads as unset, the shell convention,
    // rather than as an unparseable value. A value that is PRESENT but not
    // valid UTF-8 is neither unset nor a size, so it is reported rather than
    // collapsed into "absent" by `env::var`'s single `Err` for both -- silently
    // applying the default cap under a value the operator did set is exactly
    // the failure DSC-105 refuses (spec: DSC-111).
    //
    // The report waits for precedence, though (spec: DSC-104): an undecodable
    // value is refused only when the environment is the origin that would have
    // been USED. A junk-but-decodable value is never even parsed when the flag
    // is present (`config::resolve_metadata_limit_detail` picks one origin and parses
    // only that), so refusing the undecodable one earlier would make the
    // flag-outranks-the-environment rule depend on which bytes happen to be in
    // a variable the flag was supposed to override.
    //
    // spec: DSC-113 -- `completions` and `man` read no metadata at all, so the
    // whole resolution is skipped for them: a malformed flag, environment
    // variable, or config value must not stop a shell from loading its
    // completion script or a user from reading the manual.
    if !matches!(cli.command, Command::Completions { .. } | Command::Man) {
        install_metadata_limit(&cli, &paths)?;
        configure_hook_log(&cli, &paths);
    }

    // spec: STO-40 STO-41 STO-42
    // Completions and man touch no persisted state: skip the lock. All other
    // commands acquire the lock (shared or exclusive) before reading or writing.
    let dry_run = is_dry_run(&cli.command);
    match lock_mode(&cli.command, cli.json, cli.ascii) {
        LockMode::None => dispatch(cli, &paths),
        LockMode::Exclusive => {
            let mut lock = lock::open(&paths)?;
            let _guard = lock.write()?;
            // spec: STO-87 LIFE-61
            if dry_run {
                transaction::refuse_pending(&paths)?;
            }
            transaction::recover(&paths)?;
            // spec: LIFE-59 LIFE-62 -- one transaction per mutating command:
            // committed by each manifest save and at a successful end, rolled
            // back when the command fails.
            let tx = transaction::begin(&paths);
            let result = dispatch(cli, &paths);
            tx.settle(&result)?;
            result
        }
        LockMode::Shared => {
            let mut lock = lock::open(&paths)?;
            loop {
                {
                    let _guard = lock.read()?;
                    if !transaction::pending(&paths)? {
                        return dispatch(cli, &paths);
                    }
                    if dry_run {
                        transaction::refuse_pending(&paths)?;
                    }
                }
                let _guard = lock.write()?;
                transaction::recover(&paths)?;
            }
        }
    }
}

/// Set up this run's hook results (HOOK-163..HOOK-165): the history file and
/// its cap from the `hook-history` key, and output logging from `--hook-log`,
/// else the `hook-log` key. A config that will not parse leaves the defaults
/// (history on, no logs), with a warning when it names a hook key.
// spec: HOOK-164 HOOK-165
fn configure_hook_log(cli: &Cli, paths: &Paths) {
    // spec: HOOK-164 -- not every verb reads config for anything else (a
    // source's `hooks run` does not), so when the file names a hook key, say
    // here that it was not applied rather than drop it silently.
    let cfg = config::Config::load(paths).unwrap_or_else(|e| {
        let names_hook_key = std::fs::read_to_string(paths.config_file())
            .map(|t| {
                t.contains("hook-history")
                    || t.contains("hook_history")
                    || t.contains("hook-log")
                    || t.contains("hook_log")
            })
            .unwrap_or(false);
        if names_hook_key {
            eprintln!("warning: hook settings in config.toml not applied, using the defaults: {e}");
        }
        config::Config::default()
    });
    let cap = cfg.hook_history.unwrap_or(hook_log::DEFAULT_HISTORY);
    // Anchored like a named directory, so a relative `MIND_HOME` still gives
    // absolute log paths (HOOK-165) and the ownership check compares like with
    // like (HOOK-166).
    let owned = owned_dir(paths);
    let owned_logs = owned.clone();
    let history = hook_log::anchor(&hook_log::history_file(&paths.mind_home));
    // Clap refuses an empty `--hook-log=`.
    let logs = match &cli.hook_log {
        // spec: CLI-256
        _ if cli.no_hook_log => None,
        Some(None) => Some(owned),
        // `--hook-log=~/x` needs the `=`, and the shell leaves a `~` after
        // one unexpanded, so expand it here as the config key is (CLI-252).
        Some(Some(dir)) => Some(hook_log::anchor(&commands::expand_tilde(
            &dir.to_string_lossy(),
        ))),
        None => hook_log::config_logs(cfg.hook_log.as_ref(), &owned, &paths.mind_home),
    };
    // spec: HOOK-170 -- the flag names logs to keep, so it also turns logging
    // on (to the owned directory) when nothing else did; the config key only
    // shapes logging that is on.
    let logs = match (logs, cli.hook_log_keep) {
        (None, Some(_)) => Some(owned_dir(paths)),
        (logs, _) => logs,
    };
    let keep = cli.hook_log_keep.or(cfg.hook_log_keep).unwrap_or_default();
    hook_log::configure(hook_log::RunConfig {
        verb: verb_name(&cli.command).to_string(),
        history: (cap > 0).then_some((history, cap)),
        logs,
        keep,
        owned_logs,
    });
}

/// The owned log directory, anchored (HOOK-165, HOOK-166).
fn owned_dir(paths: &Paths) -> std::path::PathBuf {
    hook_log::anchor(&hook_log::owned_logs_dir(&paths.mind_home))
}

/// Resolve this run's metadata cap from the flag, the environment, and the
/// config key (DSC-104), install it, and record where it came from (CLI-241).
/// Warns when a zero-valued spelling turned the cap off (DSC-110).
fn install_metadata_limit(cli: &Cli, paths: &Paths) -> Result<()> {
    let env_raw = std::env::var_os(MAX_METADATA_SIZE_ENV);
    let env = match &env_raw {
        None => None,
        Some(raw) => match raw.to_str() {
            Some(text) => Some(text.to_string()).filter(|v| !v.trim().is_empty()),
            // Undecodable and the environment IS the origin (no flag): refuse,
            // naming it. With a flag present it is overridden and dropped, the
            // same fate a junk-but-decodable value meets.
            None if cli.max_metadata_size.is_none() => {
                return Err(MindError::BadMetadataSize {
                    origin: MAX_METADATA_SIZE_ENV.to_string(),
                    msg: "the value is not valid UTF-8".to_string(),
                });
            }
            None => None,
        },
    };
    let configured = if cli.max_metadata_size.is_none() && env.is_none() {
        match config::Config::load(paths) {
            Ok(c) => c.max_metadata_size,
            // spec: DSC-104 -- a config that will not parse falls back to the
            // default cap, unless it names the cap key: then it is the origin
            // that would have been used, and silently applying the default
            // under a value the operator set is refused (DSC-105).
            Err(e) => {
                let names_cap = std::fs::read_to_string(paths.config_file())
                    .map(|t| t.contains("max-metadata-size") || t.contains("max_metadata_size"))
                    .unwrap_or(false);
                if names_cap {
                    return Err(e);
                }
                None
            }
        }
    } else {
        None
    };
    let (limit, origin) = config::resolve_metadata_limit_detail(
        cli.max_metadata_size.as_deref(),
        env.as_deref(),
        configured.as_deref(),
    )?;
    error::set_metadata_size_limit(limit);
    // spec: DSC-110 -- a zero spelling is "no ceiling", which is more often a
    // typo for a small cap than a choice; say so on stderr (never stdout, so a
    // `--json` document stays parseable) and keep going. The words `unlimited`
    // and `none` say it on purpose and stay silent.
    if let Some(raw) = origin.raw()
        && config::zero_turns_cap_off(raw)
    {
        eprintln!(
            "warning: {} value '{}' is zero, which turns mind's metadata cap off (no ceiling); \
             write 'unlimited' to do that on purpose, or give a size to keep a cap",
            origin.label(),
            sanitize::strip_ansi(raw)
        );
    }
    config::set_metadata_limit_origin(origin);
    Ok(())
}

/// Run the verb, then offer the items hooks it made pending (HOOK-191): the
/// manifest is snapshotted before a verb that can change installed items, and
/// a source whose items differ afterwards has its pending items hooks offered
/// once. A verb that fails offers none; a later `upgrade` catches up.
// spec: HOOK-191
fn dispatch(mut cli: Cli, paths: &Paths) -> Result<()> {
    // spec: CLI-262 HOOK-191 -- resolve `upgrade`'s positional target before
    // the settle scope is taken, so `mind upgrade <source>` catches up that
    // source's pending items hooks as `--source <source>` does.
    if let Command::Upgrade { item, source, .. } = &mut cli.command {
        let (i, s) = commands::upgrade_target(paths, item.take(), source.take())?;
        *item = i;
        *source = s;
    }
    let settle = items_settle(&cli.command);
    // spec: HOOK-198 -- the verb's plan answer covers the items hooks the
    // settle below offers, so the answers outlive the verb's own scope.
    let _answers = settle.as_ref().map(|_| consent::scope());
    let before = settle.as_ref().map(|_| items_hook::snapshot(paths));
    dispatch_verb(cli, paths)?;
    if let (Some((verb, scope, accept)), Some(before)) = (settle, before) {
        items_hook::settle(paths, &before, verb, &scope, accept)?;
    }
    Ok(())
}

fn dispatch_verb(cli: Cli, paths: &Paths) -> Result<()> {
    // Global flags sourced before the match moves cli.command.
    let json = cli.json;
    let yes = cli.yes;
    let ascii = cli.ascii;
    match cli.command {
        Command::Meld(args) => commands::meld_verb(paths, args, yes, json),
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
            dry_run,
            uninstall_hook,
            accept,
        } => {
            if dry_run {
                commands::unmeld_preview(paths, &name, keep_items, accept.accept(), uninstall_hook)
            } else {
                commands::unmeld(
                    paths,
                    &name,
                    keep_items,
                    yes,
                    accept.accept(),
                    uninstall_hook,
                )
            }
        }
        Command::Learn {
            item,
            all,
            pin,
            kind,
            dry_run,
            force,
            overwrite,
            accept,
            local,
            profiles,
            no_default_profile,
            all_profiles,
            home,
            lobe,
        } => {
            // spec: PROF-33 -- `--profile` with no value lists the source's
            // profiles; an empty value beside named ones is ignored.
            let list_profiles = !profiles.is_empty()
                && profiles.iter().all(|p| p.trim().is_empty())
                && !all_profiles;
            let profiles: Vec<String> = profiles
                .into_iter()
                .filter(|p| !p.trim().is_empty())
                .collect();
            let profile_names = crate::profile::parse_cli_names(&profiles)?;
            let profile_flags =
                crate::profile::flags_given(&profile_names, no_default_profile, all_profiles);
            // spec: HARN-20/HARN-21 -- scope the install to the project lobe when
            // `--local`; guard held for the whole arm (URL, dry-run, glob paths).
            let _local_guard = commands::apply_local_scope(paths, local)?;
            let flow = commands::InstallFlow {
                yes,
                // spec: CLI-246 -- `--overwrite` accepts every conflict.
                clobber: if force {
                    commands::Clobber::Force
                } else if overwrite {
                    commands::Clobber::Overwrite
                } else {
                    commands::Clobber::Prompt
                },
                accept: accept.accept(),
            };
            // spec: LNK-6 -- a deep tree/blob URL one-shots: register the
            // item-link instance, then install its skill. spec: CLI-200 -- `--pin`
            // freezes the link's ref while registering.
            if item.contains("://") {
                // spec: PROF-8 -- an item link is a single item: no profiles.
                crate::profile::reject_on_item_link(&profile_flags)?;
                let item_kind = commands::parse_link_kind(kind, &item)?;
                return commands::learn_link(
                    paths,
                    &item,
                    pin,
                    item_kind,
                    home.as_deref(),
                    lobe.as_deref(),
                    dry_run,
                    flow,
                );
            }
            // spec: HOME-12 LOBE-7 -- `--home` and `--lobe` only apply to a
            // source `learn` registers.
            for (flag, given) in [("--home", home.is_some()), ("--lobe", lobe.is_some())] {
                if given {
                    eprintln!(
                        "note: {flag} is ignored for '{}'; it applies only to a deep tree/blob URL",
                        crate::sanitize::strip_ansi(&item)
                    );
                }
            }
            // spec: CLI-239 -- `--kind` describes an item link's file, so a
            // plain item ref (which names an already-melded source's item, whose
            // kind is already known) is a usage error, not a silent no-op.
            if let Some(k) = kind {
                return Err(crate::error::MindError::BadKindFlag {
                    value: k.to_kind().as_str().to_string(),
                    reason: format!(
                        "'{}' is an item ref, not an item link; --kind applies to a deep \
                         tree/blob URL only",
                        crate::sanitize::strip_ansi(&item)
                    ),
                });
            }
            // spec: CLI-200 -- `--pin` only applies to a deep-link URL; for a
            // plain item ref (an already-melded source) it is a no-op, noted here.
            if pin {
                eprintln!(
                    "note: --pin is ignored for '{item}'; it applies only to a deep tree/blob URL"
                );
            }
            if no_default_profile && !json {
                eprintln!(
                    "note: --no-default-profile changes nothing for learn, which enables only \
                     the profiles it names"
                );
            }
            if list_profiles {
                return commands::list_profiles(paths, Some(&item), json);
            }
            // spec: PROF-17 -- `learn <source> --profile/--all-profiles`
            // installs those profiles of an already-melded source.
            if !profile_names.is_empty() || all_profiles {
                let selection = crate::profile::Selection {
                    include_default: false,
                    named: profile_names,
                    all: all_profiles,
                };
                return commands::learn_source_profiles(paths, &item, &selection, dry_run, flow);
            }
            // CLI-36: `--all` rewrites the ref into the `<source>#*` selector.
            let item = if all {
                resolve::all_selector(&item)?
            } else {
                item
            };
            commands::learn(paths, &item, dry_run, flow)?;
            // spec: PROF-20 -- a by-name learn marks the item explicit.
            if !all && !dry_run {
                commands::mark_learned_explicit(paths, &item)?;
            }
            Ok(())
        }
        Command::Forget {
            item,
            unmanaged,
            force: _,
            dry_run,
            dropped: _,
            profile: Some(profile),
            accept,
        } if !unmanaged && dry_run => commands::forget_source_profile_preview(
            paths,
            item.as_deref().unwrap_or_default(),
            &profile,
            accept.accept(),
        ),
        Command::Forget {
            item,
            unmanaged: false,
            profile: Some(profile),
            accept,
            ..
        } => commands::forget_source_profile(
            paths,
            item.as_deref().unwrap_or_default(),
            &profile,
            yes,
            accept.accept(),
        ),
        Command::Forget {
            item,
            unmanaged,
            force,
            dry_run,
            dropped,
            profile: _,
            accept,
        } => {
            if dry_run {
                commands::forget_preview(
                    paths,
                    item.as_deref(),
                    unmanaged,
                    dropped,
                    accept.accept(),
                )
            } else {
                commands::forget(
                    paths,
                    item.as_deref(),
                    unmanaged,
                    dropped,
                    yes,
                    force,
                    accept.accept(),
                )
            }
        }
        // spec: CLI-261
        Command::Prune {
            source,
            dry_run,
            accept,
        } => {
            if dry_run {
                commands::prune_preview(paths, source.as_deref(), accept.accept())
            } else {
                commands::prune(paths, source.as_deref(), yes, accept.accept())
            }
        }
        Command::Sync {
            source,
            upgrade,
            skip_new,
            ignore_new,
            accept,
        } => {
            // spec: LIFE-55
            commands::set_new_items(commands::NewItems::from_flags(skip_new, ignore_new));
            commands::sync_with_selector(paths, source.as_deref(), upgrade, yes, accept.accept())
        }
        Command::Upgrade {
            item,
            source,
            no_sync,
            dry_run,
            allow_breaking,
            skip_new,
            ignore_new,
            accept,
        } => {
            let accept = accept.accept();
            // spec: AUP-21
            if !json && !dry_run {
                auto_update::show_unshown(paths);
            }
            // spec: VER-63
            commands::set_allow_breaking(allow_breaking);
            // spec: LIFE-55
            commands::set_new_items(commands::NewItems::from_flags(skip_new, ignore_new));
            if dry_run {
                return commands::upgrade_preview(
                    paths,
                    item.as_deref(),
                    source.as_deref(),
                    accept,
                );
            }
            // spec: CLI-262 - `dispatch` already resolved a target naming no
            // installed item but a melded source to that `--source` filter.
            // spec: CLI-169 - default syncs first; --no-sync skips the fetch.
            if let Some(source) = source {
                // spec: CLI-244 - scope the sync and the pass to one source.
                commands::upgrade_source(paths, yes, item.as_deref(), &source, no_sync, accept)
            } else if no_sync {
                commands::upgrade_no_sync(paths, yes, item.as_deref(), accept)
            } else {
                commands::upgrade(paths, yes, item.as_deref(), accept)
            }
        }
        Command::Evolve { check, version } => selfupdate::run(check, yes, version),
        Command::Recall {
            sources,
            item,
            kind,
            source,
            tree,
            profiles,
        } => {
            // spec: AUP-21
            if !json {
                auto_update::show_unshown(paths);
            }
            // spec: PROF-31
            if let Some(source_ref) = profiles {
                return commands::list_profiles(paths, source_ref.as_deref(), json);
            }
            commands::recall(
                paths,
                sources,
                item.as_deref(),
                kind.map(|k| k.to_kind()),
                source.as_deref(),
                json,
                tree,
            )
        }
        Command::Probe {
            query,
            kind,
            source,
            no_tui,
        } => {
            if probe_launches_tui(no_tui, json, ascii) {
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
                commands::review_policy_dispatch(&p)
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
                LobesCmd::Add {
                    path,
                    preset,
                    subdir,
                    snapshot,
                    force,
                } => commands::lobe_add_resolved(
                    paths,
                    path.as_deref(),
                    preset.as_deref(),
                    subdir.as_deref(),
                    snapshot,
                    force,
                    yes,
                ),
                LobesCmd::List => commands::lobe_list(paths),
                LobesCmd::Detect => commands::lobe_detect(paths, yes),
                LobesCmd::Remove { path, snapshot } => {
                    commands::lobe_remove(paths, &path, snapshot)
                }
            },
            ConfigCmd::AutoUpdate { state, every } => {
                commands::config_auto_update(paths, state == OnOff::On, &every)
            }
        },
        Command::AutoUpdate {
            action:
                AutoUpdateCmd::Run {
                    if_stale,
                    background,
                },
        } => commands::auto_update_run(paths, if_stale.as_deref(), background, json),
        Command::LinkProject {
            dir,
            preset,
            subdir,
            snapshot,
            force,
        } => {
            // Default preset is devin (spec: HARN-11 CLI-198).
            let preset_name = preset.as_deref().unwrap_or("devin");
            let (effective_preset, effective_subdir) = if subdir.is_some() {
                (None, subdir.as_deref())
            } else {
                (Some(preset_name), None)
            };
            commands::lobe_add_resolved(
                paths,
                dir.as_deref(),
                effective_preset,
                effective_subdir,
                snapshot,
                force,
                yes,
            )
        }
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
                accept,
            } => hooks_cmd::run(paths, &target, event, force, accept.accept()),
            HooksCmd::List { target } => hooks_cmd::list(paths, &target),
            HooksCmd::History {
                target,
                failed,
                outcome,
                event,
                since,
                limit,
                show,
            } => hooks_cmd::history(
                paths,
                &hooks_cmd::HistoryQuery {
                    target,
                    failed,
                    outcomes: outcome,
                    events: event,
                    since,
                    limit,
                    show,
                },
            ),
        },
        // spec: CUR-1 -- one pass over every registered curator.
        Command::Curate {
            check,
            prune,
            no_sync,
            adopt,
            accept,
        } => curate::run(
            paths,
            curate::CurateFlags {
                check,
                // spec: CUR-9 -- the global `--yes` is the apply gate, as it is
                // for every other prompting verb.
                yes,
                prune,
                no_sync,
                adopt,
                accept: accept.accept(),
            },
        ),
        Command::Dump {
            output,
            whole_sources,
            ranges,
        } => dump::run(paths, output, whole_sources, ranges),
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
        lock_mode(&cli.command, cli.json, cli.ascii)
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
        assert_eq!(mode_of(&["mind", "prune"]), LockMode::Exclusive);
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

    /// Under `--json`, stdout is reserved for the one result document (CLI-217)
    /// for every verb that answers with one, and NOT for the verbs whose stdout
    /// carries something else. Classified at the parse layer so both halves are
    /// visible in one place: an end-to-end test can only ever cover the verbs
    /// someone thought to list, and the reservation's failure mode for an
    /// excluded verb is silent (its output goes to stderr and stdout is empty).
    /// CLI-218's closed exclusion list is exactly the second group below;
    /// `review` and `hooks list` moved OUT of it (they answer `--json` with a
    /// document now, CLI-219/CLI-220) and `config show` moved out too (it
    /// always had a JSON branch; CLI-217's prior classification of it was
    /// simply wrong, see `config_show_json_emits_one_document` in tests/cli/output.rs).
    // spec: CLI-217 CLI-218
    #[test]
    fn json_reservation_covers_result_verbs_and_spares_payload_verbs() {
        fn reserves(args: &[&str]) -> bool {
            let cli = Cli::try_parse_from(args).expect("args should parse");
            json_reserves_stdout(&cli.command)
        }

        // Answers `--json` with a CLI-153 result object (or, for `hooks run`,
        // with the CLI-181 error envelope and nothing else).
        for args in [
            &["mind", "meld", "owner/repo"][..],
            &["mind", "unmeld", "src"],
            &["mind", "learn", "review"],
            &["mind", "forget", "review"],
            &["mind", "prune"],
            &["mind", "sync"],
            &["mind", "sync", "--upgrade"],
            &["mind", "upgrade"],
            &["mind", "absorb", "skill:review"],
            &["mind", "recall"],
            &["mind", "probe"],
            &["mind", "introspect"],
            &["mind", "config", "lobes", "add", "/some/home"],
            &["mind", "config", "lobes", "list"],
            &["mind", "config", "show"],
            &["mind", "link-project"],
            &["mind", "hooks", "run", "agents"],
            &["mind", "hooks", "list", "agents"],
            &["mind", "hooks", "history"],
            &["mind", "review", "/some/path"],
            &["mind", "review", "--policy", "/etc/mind/policy.toml"],
        ] {
            assert!(
                reserves(args),
                "{args:?} answers --json with a document, so stdout must be reserved"
            );
        }

        // stdout is a non-JSON product, or (init-source) the verb has no
        // `--json` output at all. Reserving would redirect what they print
        // into stderr. This is CLI-218's closed exclusion list.
        for args in [
            &["mind", "dump"][..],
            &["mind", "completions", "bash"],
            &["mind", "man"],
            &["mind", "evolve", "--check"],
            &["mind", "init-source"],
        ] {
            assert!(
                !reserves(args),
                "{args:?} writes something other than a JSON document to stdout"
            );
        }
    }

    /// One representative argv per INVOCABLE command, i.e. per leaf of clap's
    /// command tree (`config lobes add`, not just `config`). Keyed by the
    /// subcommand path so the CLI-218 gate below can cross-check the table
    /// against clap's own tree in BOTH directions.
    ///
    /// `json_reserves_stdout` matches on nested variants
    /// (`Command::Hooks { action: HooksCmd::List }` was an exclusion until
    /// CLI-220), so the classification has to be made at leaf granularity or a
    /// new `config`/`hooks` child could opt out unnoticed.
    const VERB_SAMPLES: &[(&[&str], &[&str])] = &[
        (&["meld"], &["mind", "meld", "owner/repo"]),
        (&["unmeld"], &["mind", "unmeld", "src"]),
        (&["learn"], &["mind", "learn", "review"]),
        (&["forget"], &["mind", "forget", "review"]),
        (&["prune"], &["mind", "prune"]),
        (&["sync"], &["mind", "sync"]),
        (&["upgrade"], &["mind", "upgrade"]),
        (&["recall"], &["mind", "recall"]),
        (&["probe"], &["mind", "probe"]),
        (&["introspect"], &["mind", "introspect"]),
        (&["absorb"], &["mind", "absorb", "skill:review"]),
        (&["link-project"], &["mind", "link-project"]),
        (&["review"], &["mind", "review", "/some/path"]),
        (
            &["review"],
            &["mind", "review", "--policy", "/etc/mind/policy.toml"],
        ),
        (&["config", "show"], &["mind", "config", "show"]),
        (
            &["config", "lobes", "add"],
            &["mind", "config", "lobes", "add", "/some/home"],
        ),
        (
            &["config", "lobes", "list"],
            &["mind", "config", "lobes", "list"],
        ),
        (
            &["config", "lobes", "detect"],
            &["mind", "config", "lobes", "detect"],
        ),
        (
            &["config", "lobes", "remove"],
            &["mind", "config", "lobes", "remove", "/some/home"],
        ),
        (
            &["config", "auto-update"],
            &["mind", "config", "auto-update", "on"],
        ),
        (&["auto-update", "run"], &["mind", "auto-update", "run"]),
        (&["curate"], &["mind", "curate", "--check"]),
        (&["hooks", "run"], &["mind", "hooks", "run", "agents"]),
        (&["hooks", "list"], &["mind", "hooks", "list", "agents"]),
        (&["hooks", "history"], &["mind", "hooks", "history"]),
        // The CLI-218 exclusions.
        (&["dump"], &["mind", "dump"]),
        (&["completions"], &["mind", "completions", "bash"]),
        (&["man"], &["mind", "man"]),
        (&["evolve"], &["mind", "evolve", "--check"]),
        (&["init-source"], &["mind", "init-source"]),
    ];

    /// CLI-218's exclusion list, spelled by subcommand path. Everything else
    /// answers `--json` with exactly one document.
    const CLI_218_EXCLUSIONS: &[&[&str]] = &[
        &["dump"],
        &["completions"],
        &["man"],
        &["evolve"],
        &["init-source"],
    ];

    /// Every invocable command path in clap's tree: the leaves, so
    /// `config lobes add` rather than `config`. clap's synthesized `help`
    /// subcommands are not mind verbs and are skipped.
    fn clap_command_paths(cmd: &clap::Command, prefix: &[String]) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            if name == "help" {
                continue;
            }
            let mut path = prefix.to_vec();
            path.push(name.to_string());
            let children = clap_command_paths(sub, &path);
            if children.is_empty() {
                out.push(path);
            } else {
                out.extend(children);
            }
        }
        out
    }

    /// The CLI-218 boundary as a GATE rather than a restatement: enumerate the
    /// commands from clap itself (`Cli::command()`), not from a list someone
    /// maintains by hand, and require every one of them to be classified. A
    /// verb (or a new `config`/`hooks` child) added tomorrow has no
    /// `VERB_SAMPLES` row, so this test goes red and its author has to decide,
    /// explicitly, whether it answers `--json` with a document or joins the
    /// closed exclusion list. That is exactly what the CLI-218 statement buys,
    /// and what the end-to-end
    /// `cli_218_every_driven_verb_is_json_or_a_named_exclusion` (a fixed list
    /// of invocations, which a new verb simply would not appear in) cannot do.
    // spec: CLI-217 CLI-218
    #[test]
    fn cli_218_boundary_is_closed_over_every_clap_subcommand() {
        let cmd = Cli::command();
        let paths = clap_command_paths(&cmd, &[]);
        assert!(
            paths.len() > 10,
            "the walk must actually find the command tree, got {paths:?}"
        );
        let has_sample = |path: &[String]| {
            VERB_SAMPLES
                .iter()
                .any(|(p, _)| p.len() == path.len() && p.iter().zip(path).all(|(a, b)| a == b))
        };

        // 1. Every command clap knows about is classified here.
        for path in &paths {
            assert!(
                has_sample(path),
                "`mind {}` has no CLI-218 classification: add a VERB_SAMPLES row \
                 and either give it a `--json` document or add it to \
                 CLI_218_EXCLUSIONS (and to spec/cli.md's CLI-218 list)",
                path.join(" ")
            );
        }
        // 2. ...and nothing here names a command clap dropped.
        for (path, _) in VERB_SAMPLES {
            assert!(
                paths
                    .iter()
                    .any(|p| p.len() == path.len() && p.iter().zip(*path).all(|(a, b)| a == b)),
                "VERB_SAMPLES names `mind {}`, which is not a clap command any more",
                path.join(" ")
            );
        }
        // 3. Every exclusion is a real command (a typo would silently widen the
        //    "answers with a document" side otherwise).
        for path in CLI_218_EXCLUSIONS {
            assert!(
                paths
                    .iter()
                    .any(|p| p.len() == path.len() && p.iter().zip(*path).all(|(a, b)| a == b)),
                "CLI_218_EXCLUSIONS names `mind {}`, which is not a clap command",
                path.join(" ")
            );
        }
        // 4. The reservation agrees with the classification, command by command.
        for (path, args) in VERB_SAMPLES {
            let cli = Cli::try_parse_from(*args).expect("sample args should parse");
            let reserves = json_reserves_stdout(&cli.command);
            let excluded = CLI_218_EXCLUSIONS.contains(path);
            assert_eq!(
                reserves,
                !excluded,
                "`mind {}`: json_reserves_stdout said {reserves}, but CLI-218 \
                 classifies it as {}",
                path.join(" "),
                if excluded {
                    "an exclusion"
                } else {
                    "answering with a document"
                }
            );
        }
    }

    /// The verb table at the top of docs/src/commands.md names every visible
    /// top-level verb clap knows, so a new verb cannot ship without a row.
    #[test]
    fn the_commands_doc_lists_every_visible_verb() {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/commands.md"),
        )
        .expect("read docs/src/commands.md");
        let table: Vec<&str> = doc
            .lines()
            .skip_while(|l| !l.starts_with("| verb |"))
            .take_while(|l| l.starts_with('|'))
            .collect();
        assert!(table.len() > 10, "found the verb table: {table:?}");
        let cmd = Cli::command();
        for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
            let name = format!("`{}`", sub.get_name());
            assert!(
                table.iter().any(|row| row.contains(&name)),
                "docs/src/commands.md's verb table has no row for {name}"
            );
        }
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
        // spec: HOOK-167
        assert_eq!(
            mode_of(&["mind", "hooks", "history"]),
            LockMode::Shared,
            "hooks history only reads, so it takes the shared lock"
        );
    }

    /// `hooks run --event` accepts install, uninstall, build value variants and
    /// parses correctly. `--force` and `--accept-hooks` parse as expected.
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
                        accept,
                    },
            } => {
                assert_eq!(target, "agents");
                assert_eq!(event, HookEventArg::Install);
                assert!(!force);
                assert_eq!(accept.accept(), crate::hook::Accept::default());
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
            "--accept-hooks",
        ])
        .expect("hooks run with all flags should parse");
        match cli.command {
            Command::Hooks {
                action:
                    HooksCmd::Run {
                        event,
                        force,
                        accept,
                        ..
                    },
            } => {
                assert_eq!(event, HookEventArg::Build);
                assert!(force);
                assert_eq!(accept.accept(), crate::hook::Accept::ALL);
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

    /// `sync --upgrade --accept-hooks` must parse and the flag must be forwarded
    /// to the upgrade pass so non-TTY CI can trigger hook re-runs unattended
    /// (HOOK-11, HOOK-23). Verified here by inspecting the parsed struct; the
    /// end-to-end behavior is covered by tests/cli/.
    // spec: HOOK-11 HOOK-23 CLI-250
    #[test]
    fn sync_accept_hooks_parses() {
        let sync_accept = |args: &[&str]| match Cli::try_parse_from(args)
            .expect("sync should parse")
            .command
        {
            Command::Sync {
                upgrade, accept, ..
            } => {
                assert!(upgrade, "--upgrade should be true");
                accept.accept()
            }
            other => panic!("expected Sync, got {other:?}"),
        };
        assert_eq!(
            sync_accept(&["mind", "sync", "--upgrade"]),
            crate::hook::Accept::default()
        );
        assert_eq!(
            sync_accept(&["mind", "sync", "--upgrade", "--accept-hooks"]),
            crate::hook::Accept::ALL
        );

        // Every spelling requires --upgrade so it cannot be a silent no-op.
        for flag in [
            "--accept-hooks",
            "--accept-hooks=install",
            "--dangerously-skip-install-hook-check",
            "--dangerously-skip-hook-check",
            "--dangerously-skip-build-hook-check",
        ] {
            assert!(
                Cli::try_parse_from(["mind", "sync", flag]).is_err(),
                "sync {flag} without --upgrade must be a parse error"
            );
        }

        // Confirm the lock mode is still Exclusive with the flag.
        assert_eq!(
            mode_of(&["mind", "sync", "--upgrade", "--accept-hooks"]),
            LockMode::Exclusive,
            "sync --upgrade --accept-hooks must take the exclusive lock"
        );
    }

    /// `--accept-hooks` alone accepts every event, `=a,b` (or a repeat) only
    /// the named ones, an unknown event is a usage error, and the bare form
    /// does not swallow a following positional.
    // spec: CLI-250
    #[test]
    fn accept_hooks_parses_events() {
        use crate::hook::Accept;
        let upgrade_accept = |args: &[&str]| match Cli::try_parse_from(args)
            .expect("upgrade should parse")
            .command
        {
            Command::Upgrade { accept, .. } => accept.accept(),
            other => panic!("expected Upgrade, got {other:?}"),
        };
        assert_eq!(upgrade_accept(&["mind", "upgrade"]), Accept::default());
        assert_eq!(
            upgrade_accept(&["mind", "upgrade", "--accept-hooks"]),
            Accept::ALL
        );
        assert_eq!(
            upgrade_accept(&["mind", "upgrade", "--accept-hooks=install,build"]),
            Accept {
                install: true,
                build: true,
                ..Accept::default()
            }
        );
        assert_eq!(
            upgrade_accept(&[
                "mind",
                "upgrade",
                "--accept-hooks=update",
                "--accept-hooks=uninstall",
            ]),
            Accept {
                update: true,
                uninstall: true,
                ..Accept::default()
            }
        );
        assert!(
            Cli::try_parse_from(["mind", "upgrade", "--accept-hooks=deploy"]).is_err(),
            "an unknown event must be a usage error"
        );
        assert!(
            Cli::try_parse_from(["mind", "upgrade", "--accept-hooks="]).is_err(),
            "an empty event list must be a usage error"
        );

        // The bare flag takes no value, so the next word stays the positional.
        match Cli::try_parse_from(["mind", "upgrade", "--accept-hooks", "review"])
            .expect("upgrade --accept-hooks <item> should parse")
            .command
        {
            Command::Upgrade { item, accept, .. } => {
                assert_eq!(item.as_deref(), Some("review"));
                assert_eq!(accept.accept(), Accept::ALL);
            }
            other => panic!("expected Upgrade, got {other:?}"),
        }

        // Every verb that runs hooks takes the flag.
        for args in [
            &["mind", "meld", "owner/repo", "--accept-hooks"][..],
            &["mind", "learn", "review", "--accept-hooks"],
            &["mind", "forget", "review", "--accept-hooks"],
            &["mind", "unmeld", "owner/repo", "--accept-hooks"],
            &["mind", "upgrade", "--accept-hooks"],
            &["mind", "sync", "--upgrade", "--accept-hooks"],
            &["mind", "curate", "--accept-hooks"],
            &["mind", "hooks", "run", "repo", "--accept-hooks"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?} should parse");
        }
    }

    /// The pre-rename `--dangerously-skip-*` spellings still parse, hidden from
    /// `--help`, and accept exactly what they gated before.
    // spec: CLI-251 CLI-227
    #[test]
    fn legacy_hook_flags_map_onto_accept_hooks() {
        use crate::hook::Accept;
        let lifecycle = Accept {
            install: true,
            update: true,
            uninstall: true,
            build: false,
            items: false,
        };
        let upgrade_accept = |flag: &str| match Cli::try_parse_from(["mind", "upgrade", flag])
            .expect("legacy flag should parse")
            .command
        {
            Command::Upgrade { accept, .. } => accept.accept(),
            other => panic!("expected Upgrade, got {other:?}"),
        };
        assert_eq!(
            upgrade_accept("--dangerously-skip-install-hook-check"),
            lifecycle
        );
        assert_eq!(upgrade_accept("--dangerously-skip-hook-check"), lifecycle);
        assert_eq!(
            upgrade_accept("--dangerously-skip-build-hook-check"),
            Accept {
                build: true,
                ..Accept::default()
            }
        );
        // CLI-227: `forget`/`unmeld` keep both of their old spellings.
        for flag in [
            "--dangerously-skip-hook-check",
            "--dangerously-skip-install-hook-check",
        ] {
            match Cli::try_parse_from(["mind", "forget", "review", flag])
                .expect("legacy forget flag should parse")
                .command
            {
                Command::Forget { accept, .. } => assert!(accept.accept().uninstall),
                other => panic!("expected Forget, got {other:?}"),
            }
        }

        let mut cmd = Cli::command();
        let upgrade = cmd
            .find_subcommand_mut("upgrade")
            .expect("upgrade subcommand");
        let help = upgrade.render_long_help().to_string();
        assert!(help.contains("--accept-hooks"), "{help}");
        assert!(!help.contains("dangerously"), "{help}");
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
