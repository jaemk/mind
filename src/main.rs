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
mod ignore;
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

use clap::{CommandFactory, Parser};

use cli::{Cli, Command, ConfigCmd, HooksCmd, LobesCmd};
use error::Result;
use paths::Paths;

/// CLI-217's enforcement mechanism: under `--json`, stdout is RESERVED for the
/// one result document the invoked verb answers with.
///
/// Three rounds of per-site sweeps each shipped a fresh leak into `--json`
/// stdout (an unguarded `println!` ahead of the result, a nested verb's second
/// result object, a hook's own output), so the rule is enforced structurally
/// here instead of by discipline at every call site:
///
/// 1. [`reserve`] moves the real stdout aside and points fd 1 at stderr for the
///    rest of the process. After that NOTHING can reach stdout by printing:
///    not a `println!` anywhere in the tree, not a `print!` in a module this
///    change never touched, and not a child process that inherits fd 1 (a
///    source's install hook, whose output is arbitrary text chosen by the
///    source author -- the worst case in the class).
/// 2. Result documents are [`record`]ed rather than printed. The LAST one
///    recorded wins, which in this call graph is the outermost frame's: a
///    nested `learn` performed on `meld`'s or `sync`'s behalf records first and
///    the invoked verb records after it, so the caller is answered by the verb
///    it actually invoked (CLI-153).
/// 3. `main` writes that one document to the preserved stdout, exactly once, on
///    the success path -- or the CLI-181 error envelope INSTEAD of it on the
///    failure path, so a verb that mutated something and then failed cannot
///    emit both.
///
/// The two halves are deliberately coupled: recording (not printing) the
/// document is what lets fd 1 stay redirected for the whole run.
mod json_stdout {
    use std::io::Write;
    use std::sync::Mutex;

    /// The real stdout, moved aside by [`reserve`]. `None` when stdout was never
    /// reserved: text mode, or a verb whose stdout payload is not a JSON
    /// document (see `json_reserves_stdout`).
    static REAL_STDOUT: Mutex<Option<std::fs::File>> = Mutex::new(None);

    /// The document this run will answer with; the most recently recorded one.
    static PENDING: Mutex<Option<String>> = Mutex::new(None);

    /// Structured findings a verb recorded before failing (CLI-221), folded
    /// into the CLI-181 error envelope's `details` member when present.
    /// Distinct from `PENDING`: that is the SUCCESS-path document, which
    /// `emit_instead` discards on failure; a verb that fails still wants its
    /// findings to reach the caller, hence a second slot that survives the
    /// discard.
    static ERROR_DETAILS: Mutex<Option<serde_json::Value>> = Mutex::new(None);

    /// Duplicate fd 1, then point fd 1 at fd 2. Returns the duplicate (the real
    /// stdout) on success. Mirrors the TUI's stdout-capture redirect
    /// (`tui::action::with_captured_stdout`), which does the same dance.
    #[cfg(unix)]
    fn move_stdout_aside() -> Option<std::fs::File> {
        use std::os::fd::FromRawFd;
        // Nothing should be buffered this early, but flush before the swap so a
        // stray byte cannot land on the wrong description.
        let _ = std::io::stdout().flush();
        let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if saved < 0 {
            return None;
        }
        if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
            unsafe { libc::close(saved) };
            return None;
        }
        // SAFETY: `saved` is a fresh fd owned by nobody else.
        Some(unsafe { std::fs::File::from_raw_fd(saved) })
    }

    /// Non-unix fallback: no redirect, so `--json` output stays as it was
    /// (correct for every already-guarded site, unenforced for the rest).
    #[cfg(not(unix))]
    fn move_stdout_aside() -> Option<std::fs::File> {
        None
    }

    /// Reserve stdout for this run's single JSON document. Idempotent; a failure
    /// to redirect is silently tolerated (the verb still works, it just loses
    /// the structural guarantee).
    pub fn reserve() {
        let mut real = REAL_STDOUT.lock().unwrap_or_else(|e| e.into_inner());
        if real.is_none() {
            *real = move_stdout_aside();
        }
    }

    /// Whether stdout is reserved, i.e. whether a result document must be
    /// recorded rather than printed.
    pub fn is_reserved() -> bool {
        REAL_STDOUT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Record `doc` as the document this run answers with, replacing any
    /// document an inner frame recorded earlier.
    pub fn record(doc: String) {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(doc);
    }

    /// Write `doc` (plus a trailing newline, exactly as `println!` would) to the
    /// preserved stdout, or to the process stdout when nothing was reserved.
    fn write_out(doc: &str) {
        let mut real = REAL_STDOUT.lock().unwrap_or_else(|e| e.into_inner());
        match real.as_mut() {
            Some(f) => {
                let _ = writeln!(f, "{doc}");
                let _ = f.flush();
            }
            None => println!("{doc}"),
        }
    }

    /// Emit the recorded document, if any. Called once, by `main`, on success.
    pub fn flush_pending() {
        let doc = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(doc) = doc {
            write_out(&doc);
        }
    }

    /// Emit `value` as the run's answer INSTEAD of anything recorded (CLI-181:
    /// on failure the error envelope is the one document, even if a verb
    /// already recorded a result before failing).
    pub fn emit_instead<T: serde::Serialize>(value: &T) {
        let _ = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Ok(s) = serde_json::to_string_pretty(value) {
            write_out(&s);
        }
    }

    /// Record `value` as the CLI-181 error envelope's `details` member for
    /// this run's eventual failure (CLI-221). Call before returning the
    /// error; a run that never fails, or fails without recording details,
    /// leaves this `None` and the envelope carries no `details` member at all.
    pub fn record_error_details<T: serde::Serialize>(value: &T) {
        if let Ok(v) = serde_json::to_value(value) {
            *ERROR_DETAILS.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        }
    }

    /// Build the CLI-181 error envelope for `kind`/`message`, folding in
    /// whatever was recorded via [`record_error_details`] as an optional
    /// `details` member (CLI-221). Takes (clears) the recorded details, so a
    /// second call in the same run starts clean.
    pub fn build_error_envelope(kind: &str, message: &str) -> serde_json::Value {
        let mut envelope = serde_json::json!({
            "schema": 1,
            "error": {
                "kind": kind,
                "message": message,
            }
        });
        if let Some(details) = ERROR_DETAILS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            envelope["details"] = details;
        }
        envelope
    }
}

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
                // exit code is unchanged (FAILURE = 1). Plain-text stderr output
                // is suppressed; the envelope carries the full Display message.
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
fn lock_mode(command: &Command, json: bool, ascii: bool) -> LockMode {
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
                },
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
fn json_reserves_stdout(command: &Command) -> bool {
    !matches!(
        command,
        // stdout carries a non-JSON product, `--json` or not:
        //   dump      -- DUMP-9: TOML on stdout, plus a stderr note that --json
        //                does not apply
        //   completions/man -- the script / roff page IS the output
        Command::Dump { .. } | Command::Completions { .. } | Command::Man
        // `evolve` writes its own document straight to stdout from
        // `selfupdate.rs` rather than through `commands.rs`, so redirecting fd 1
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
/// rather than cited. TUI-2/TUI-71 (fallback) are tested in tests/cli.rs.
// spec: TUI-2 TUI-71
fn probe_launches_tui(no_tui: bool, json: bool, ascii: bool) -> bool {
    !no_tui && !json && !ascii && utf8_locale() && std::io::stdout().is_terminal()
}

/// Whether the active locale advertises UTF-8 (TUI-71). Checks `LC_ALL`,
/// `LC_CTYPE`, `LANG` in that order (first set wins); returns `false` when none
/// is set (a conservative ASCII default). Mirrors `render::detect_utf8_locale`,
/// which is private to that module; kept here so the probe launch gate does not
/// depend on the color/glyph capability context (which also folds in `NO_COLOR`,
/// a signal that must NOT block the TUI -- TUI-65 renders it monochrome).
fn utf8_locale() -> bool {
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

fn run(cli: Cli) -> Result<()> {
    // Install the process-wide output context before any dispatch so that
    // render::ctx() returns real capabilities for all commands (including the
    // mutating verbs that read it internally). spec: CLI-150 CLI-151 CLI-154
    crate::render::set_ctx(crate::render::OutputCtx::detect(
        cli.json,
        cli.ascii,
        cli.verbose,
    ));

    // spec: CLI-217 -- reserve stdout BEFORE anything can print to it, so the
    // guarantee does not depend on where in the verb the first line is written.
    if cli.json && json_reserves_stdout(&cli.command) {
        json_stdout::reserve();
    }

    let paths = Paths::resolve()?;

    // spec: STO-40 STO-41 STO-42
    // Completions and man touch no persisted state: skip the lock. All other
    // commands acquire the lock (shared or exclusive) before reading or writing.
    match lock_mode(&cli.command, cli.json, cli.ascii) {
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
    let ascii = cli.ascii;
    match cli.command {
        Command::Meld {
            repo,
            alias,
            roots,
            add_roots,
            flat_skills,
            pin,
            follow_branch,
            pin_tag,
            pin_ref,
            install_hook,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
            register_only,
            learn_patterns,
            recursive,
            force,
            local,
        } => {
            // spec: HARN-20/HARN-21 -- `--local` restricts the install fan-out to
            // the registered project lobe containing the cwd; without it, plain
            // meld/learn inside such a lobe gets a one-line note that --local is
            // available. The guard lives for the whole arm so every install path
            // (fresh meld, re-meld, curated chain) is scoped.
            let _local_guard = commands::apply_local_scope(paths, local)?;
            // spec: CLI-17, CLI-200..202 -- fold the pin flag (and deprecated
            // aliases) into one request; more than one is a structured
            // ConflictingPin error rather than a clap usage string.
            let pin = commands::parse_pin_flags(pin, follow_branch, pin_tag, pin_ref)?;
            // spec: CLI-236 -- reject an unusable `--learn` value before the
            // clone, so a typo does not leave a registered source behind.
            commands::validate_learn_patterns(&learn_patterns)?;
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
            // spec: STO-58 -- the alias is part of the source identity, so the
            // instance a re-meld/install targets is `host/owner/repo@<alias>`.
            // Computed before `alias` is moved into meld/remeld.
            let source_ident =
                commands::instance_name(&repo, alias.as_deref()).unwrap_or_else(|_| repo.clone());
            // CLI-12: re-melding an already-melded source is not an error; it
            // ensures the items are installed, else reports their status.
            if commands::is_melded(paths, &repo, alias.as_deref())? {
                // spec: CLI-206 -- name every discovery flag this invocation
                // carries; remeld() cannot apply any of them (it never
                // re-clones or re-registers) and reports so explicitly.
                let mut ignored_flags: Vec<&str> = Vec::new();
                if !roots.is_empty() {
                    ignored_flags.push("--root");
                }
                if !add_roots.is_empty() {
                    ignored_flags.push("--add-root");
                }
                if flat_skills {
                    ignored_flags.push("--flat-skills");
                }
                if install_hook.is_some() {
                    ignored_flags.push("--install-hook");
                }
                // spec: CLI-209 -- unlike the discovery flags above, `--pin`
                // IS honored on a re-meld: it re-pins the source instead of
                // being a silent no-op.
                commands::remeld(
                    paths,
                    &repo,
                    alias,
                    register_only,
                    flow,
                    recursive,
                    &ignored_flags,
                    pin,
                    &learn_patterns,
                )?;
            } else {
                let meld_sum = commands::meld(
                    paths,
                    &repo,
                    alias,
                    roots,
                    add_roots,
                    flat_skills,
                    pin,
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
                        // spec: CLI-236 -- `--learn <glob>` narrows the install to
                        // the matching subset in place of the whole set.
                        let (mut inst, pend) = if learn_patterns.is_empty() {
                            commands::install_source_items_for_json(paths, &source_ident, flow)?
                        } else {
                            // spec: STO-58 -- the same instance identity the
                            // text arm installs against; the two arms of one
                            // feature must not disagree on which identity they
                            // trust, even where the two spellings coincide.
                            // The two spellings are provably equal today, and
                            // using one variable keeps a future change to
                            // either derivation from breaking only the
                            // `--learn` arm.
                            commands::install_source_items_matching_for_json(
                                paths,
                                &source_ident,
                                &learn_patterns,
                                flow,
                            )?
                        };
                        // Also walk the curated chain silently (DSC-54/55/58) --
                        // but not under `--learn`, whose patterns are an
                        // explicit, top-source-scoped selection (CLI-236).
                        if learn_patterns.is_empty() {
                            let curated = commands::install_curated_sources_for_json(
                                paths,
                                &source_ident,
                                recursive,
                                flow,
                            )?;
                            inst.extend(curated);
                        }
                        commands::emit_meld_json_result(meld_sum, inst, pend)?;
                    } else if !learn_patterns.is_empty() {
                        // spec: CLI-236 -- the named subset only: the curated
                        // chain (DSC-54/55/58) is not walked, since a pattern
                        // selects within the source being melded. Say so when
                        // `--recursive` was passed, rather than dropping it
                        // silently (the CLI-206 ignored-flag discipline).
                        commands::note_learn_ignores_recursive(recursive);
                        commands::install_source_items_matching(
                            paths,
                            &source_ident,
                            &learn_patterns,
                            flow,
                        )?;
                    } else {
                        commands::install_source_items(paths, &source_ident, flow)?;
                        // DSC-54 installs only the top-level source by default. Walk the
                        // curated chain and install each nested source the curator
                        // flagged `install = true` (DSC-58), or every nested source with
                        // `--recursive` (DSC-55).
                        commands::install_curated_sources(paths, &source_ident, recursive, flow)?;
                    }
                } else if json {
                    // register-only + json: register only, emit the meld result now.
                    commands::emit_meld_json_result(meld_sum, vec![], 0)?;
                }
            }
            // DSC-56: suggest `mind probe` after melding a curated super-source.
            commands::maybe_probe_hint(paths, &source_ident)
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
            pin,
            dry_run,
            force,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
            local,
        } => {
            // spec: HARN-20/HARN-21 -- scope the install to the project lobe when
            // `--local`; guard held for the whole arm (URL, dry-run, glob paths).
            let _local_guard = commands::apply_local_scope(paths, local)?;
            let flow = commands::InstallFlow {
                yes,
                clobber: if force {
                    commands::Clobber::Force
                } else {
                    commands::Clobber::Prompt
                },
                dangerously_skip: dangerously_skip_install_hook_check,
                dangerously_skip_build: dangerously_skip_build_hook_check,
            };
            // spec: LNK-6 -- a deep tree/blob URL one-shots: register the
            // item-link instance, then install its skill. spec: CLI-200 -- `--pin`
            // freezes the link's ref while registering.
            if item.contains("://") {
                return commands::learn_link(paths, &item, pin, dry_run, flow);
            }
            // spec: CLI-200 -- `--pin` only applies to a deep-link URL; for a
            // plain item ref (an already-melded source) it is a no-op, noted here.
            if pin {
                eprintln!(
                    "note: --pin is ignored for '{item}'; it applies only to a deep tree/blob URL"
                );
            }
            // CLI-36: `--all` rewrites the ref into the `<source>#*` selector.
            let item = if all {
                resolve::all_selector(&item)?
            } else {
                item
            };
            commands::learn(paths, &item, dry_run, flow)
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
            source,
            upgrade,
            dangerously_skip_install_hook_check,
            dangerously_skip_build_hook_check,
        } => commands::sync_with_selector(
            paths,
            source.as_deref(),
            upgrade,
            yes,
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
        },
        Command::LinkProject {
            dir,
            preset,
            subdir,
            snapshot,
            force,
        } => {
            // Default preset is windsurf (spec: HARN-11 CLI-198).
            let preset_name = preset.as_deref().unwrap_or("windsurf");
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
    /// simply wrong, see `config_show_json_emits_one_document` in tests/cli.rs).
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
        (&["hooks", "run"], &["mind", "hooks", "run", "agents"]),
        (&["hooks", "list"], &["mind", "hooks", "list", "agents"]),
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

    /// The CLI-221 `details` slot is process-global state, so its whole risk
    /// surface is "what is in there when nobody put anything in there". All of
    /// it is asserted in ONE test on purpose: `build_error_envelope` TAKES the
    /// recorded value, so two tests touching the static would race under the
    /// default parallel test runner and pass or fail by scheduling.
    ///
    /// Pins, in order: an envelope built with nothing recorded has no `details`
    /// KEY at all (not `null`, not `{}`); a recorded value lands verbatim; the
    /// take is real, so a second failure in the same run cannot inherit the
    /// first one's findings; and a second record before the take overwrites
    /// rather than accumulating.
    // spec: CLI-181 CLI-182 CLI-221
    #[test]
    fn cli_221_error_details_is_absent_unless_recorded_and_never_leaks() {
        // Nothing recorded -> no `details` member whatsoever.
        let plain = json_stdout::build_error_envelope("not-installed", "item not installed");
        assert_eq!(
            plain,
            serde_json::json!({
                "schema": 1,
                "error": {"kind": "not-installed", "message": "item not installed"}
            }),
            "an error with no recorded findings must carry no `details` member"
        );
        assert!(
            !plain.as_object().is_some_and(|o| o.contains_key("details")),
            "`details` must be ABSENT, not null: {plain}"
        );

        // Recorded -> folded in verbatim.
        json_stdout::record_error_details(&serde_json::json!({
            "action": "review", "outcome": "failed", "hard": [{"kind": "toml-parse-error"}]
        }));
        let with = json_stdout::build_error_envelope("review-failed", "1 hard finding");
        assert_eq!(with["error"]["kind"], "review-failed", "{with}");
        assert_eq!(with["details"]["outcome"], "failed", "{with}");
        assert_eq!(
            with["details"]["hard"][0]["kind"], "toml-parse-error",
            "{with}"
        );

        // ...and TAKEN: an unrelated later error cannot inherit them.
        let after = json_stdout::build_error_envelope("source-not-found", "no such source");
        assert!(
            !after.as_object().is_some_and(|o| o.contains_key("details")),
            "recorded details must not leak into the next envelope: {after}"
        );

        // Recording twice before the take replaces rather than accumulating,
        // so the caller sees the LAST verb's findings, matching `record`'s
        // outermost-frame-wins rule (CLI-153).
        json_stdout::record_error_details(&serde_json::json!({"which": "first"}));
        json_stdout::record_error_details(&serde_json::json!({"which": "second"}));
        let twice = json_stdout::build_error_envelope("review-failed", "x");
        assert_eq!(
            twice["details"],
            serde_json::json!({"which": "second"}),
            "{twice}"
        );

        // Leave the static clean for anything else in this binary.
        let _ = json_stdout::build_error_envelope("x", "y");
    }

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

    // ==========================================================================
    // CLI-217: `json_stdout::emit_instead` discards a prior recording
    // ==========================================================================

    /// Turns a re-exec of this test binary into a one-shot driver of the
    /// `record` -> `emit_instead` sequence: the "verb recorded a result, then
    /// failed" corridor CLI-181 relies on (the error envelope REPLACES
    /// whatever a verb recorded before failing, CLI-217 point 2). No verb in
    /// the current call graph reaches this in practice -- every `print_json`
    /// call site in `commands.rs` is the last statement before its function
    /// returns, so nothing can record and then still error out of the SAME
    /// call -- which is exactly why this is a direct unit test of the
    /// mechanism rather than an end-to-end one: `emit_instead`'s own contract
    /// (discard the pending recording, write only the replacement) needs to
    /// hold regardless of whether today's call graph exercises it, since a
    /// future call site (or a refactor that moves a `print_json` earlier) could
    /// start relying on it.
    const EMIT_INSTEAD_TEST: &str = "tests::emit_instead_discards_a_prior_recording";
    const EMIT_INSTEAD_MODE_ENV: &str = "MIND_TEST_EMIT_INSTEAD_MODE";

    /// `json_stdout::reserve` does a real `dup2` of this process's fd 1, which
    /// would corrupt libtest's own output capturing for every OTHER test
    /// running concurrently in this binary if called in-process. Re-executing
    /// the test binary filtered to just this test isolates the fd mutation to
    /// a throwaway child, the same technique `render::tests::note_and_warn_route_by_output_mode`
    /// uses for the same reason.
    #[test]
    fn emit_instead_discards_a_prior_recording() {
        if std::env::var_os(EMIT_INSTEAD_MODE_ENV).is_some() {
            // Child role: reserve stdout for real, record a first document,
            // then let `emit_instead` replace it, exactly as main()'s error
            // path does on a verb that recorded before failing.
            json_stdout::reserve();
            json_stdout::record(
                serde_json::json!({"schema": 1, "action": "first-recorded"}).to_string(),
            );
            json_stdout::emit_instead(&serde_json::json!({
                "error": {"kind": "replacement", "message": "the one document"}
            }));
            // A THIRD write attempt (flush_pending, as main() calls on the
            // success path) must be a no-op: emit_instead already took the
            // pending slot, so there is nothing left to flush. If this wrote
            // again, the parent would see a second JSON value tacked onto
            // stdout, which does not parse as one document.
            json_stdout::flush_pending();
            std::process::exit(0);
        }

        let exe = std::env::current_exe().expect("path of the running test binary");
        let out = std::process::Command::new(exe)
            .args([
                "--exact",
                EMIT_INSTEAD_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(EMIT_INSTEAD_MODE_ENV, "1")
            .output()
            .expect("re-exec the test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // `--nocapture` means libtest's own "running 1 test" / "test ... "
        // preamble shares this stdout ahead of our real output (it is written
        // by libtest itself, before the test body runs `reserve()`), so this
        // checks for the replacement/discard by substring rather than parsing
        // the whole capture as one JSON value.
        assert!(
            stdout.contains("\"replacement\""),
            "emit_instead's value must reach the preserved stdout: {stdout:?}"
        );
        assert!(
            !stdout.contains("first-recorded"),
            "the document recorded before the failure must be discarded, not \
             concatenated ahead of the replacement: {stdout:?}"
        );
        // The trailing `flush_pending()` call in the child must not add a
        // second occurrence of the replacement text (it would if PENDING were
        // not actually cleared by `emit_instead`, so `flush_pending` wrote it
        // again on top).
        assert_eq!(
            stdout.matches("\"replacement\"").count(),
            1,
            "the replacement document must be written exactly once, not once \
             by emit_instead and again by a later flush_pending: {stdout:?}"
        );
    }
}
