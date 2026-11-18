//! `mind auto-update` and the auto-update state shown by other verbs.

use super::*;

/// The auto-update level in `recall`'s source header (AUP-11):
/// ` auto-update compatible`. Empty when the level is `off`.
// spec: AUP-11
pub(super) fn auto_update_token(
    source: &crate::source::Source,
    cap: Option<crate::auto_update::Level>,
) -> String {
    match auto_update_level(source, cap) {
        crate::auto_update::Level::Off => String::new(),
        level => format!(" auto-update {level}"),
    }
}

/// A source's auto-update state in `--json` output (AUP-11).
#[derive(Serialize)]
pub(super) struct AutoUpdateStateJson {
    pub(super) declared: Option<crate::auto_update::Level>,
    pub(super) granted: crate::auto_update::Level,
    pub(super) effective: crate::auto_update::Level,
}

pub(super) fn auto_update_json(
    source: &crate::source::Source,
    cap: Option<crate::auto_update::Level>,
) -> AutoUpdateStateJson {
    AutoUpdateStateJson {
        declared: match &source.pin {
            Pin::Version(vp) => vp.auto_update,
            _ => None,
        },
        granted: source.auto_update.unwrap_or(crate::auto_update::Level::Off),
        effective: auto_update_level(source, cap),
    }
}

/// The managed policy, for showing auto-update levels (AUP-30). A policy that
/// fails to load is reported by the verbs that enforce it, not here.
pub(super) fn policy_for_display(paths: &Paths) -> Option<Policy> {
    paths.settings().policy().ok().flatten()
}

/// The managed policy's cap on `source`'s auto-update level (AUP-30): its
/// `[sources].auto-update`, and `compatible` for a source an `auto_meld`
/// entry pins, since `sync` puts that range back (POL-55) and a breaking move
/// would only be undone, and its items downgraded, at the next sync.
// spec: AUP-30
pub(super) fn auto_update_cap(
    policy: Option<&Policy>,
    source: &crate::source::Source,
) -> Option<crate::auto_update::Level> {
    use crate::auto_update::Level;
    let policy = policy?;
    let pinned_by_policy = policy
        .auto_meld()
        .iter()
        .any(|am| parse_spec_quiet(&am.repo).is_ok_and(|spec| spec.name == source.name));
    match (policy.auto_update_cap(), pinned_by_policy) {
        (cap, false) => cap,
        (Some(cap), true) => Some(cap.min(Level::Compatible)),
        (None, true) => Some(Level::Compatible),
    }
}

/// What `meld --auto-update[=<level>]` asks for (AUP-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoUpdateGrant {
    /// Bare `--auto-update`: the level the source declares.
    Declared,
    Level(crate::auto_update::Level),
}

pub(super) fn parse_auto_update_grant(value: &str) -> Result<AutoUpdateGrant> {
    if value == "declared" {
        return Ok(AutoUpdateGrant::Declared);
    }
    crate::auto_update::Level::parse(value)
        .map(AutoUpdateGrant::Level)
        .ok_or_else(|| MindError::BadAutoUpdate {
            value: value.to_string(),
        })
}

/// Record the consumer's auto-update grant for `source_name` (AUP-8), or,
/// on the meld that registers a source declaring a level, ask for one
/// (AUP-9). A grant on anything but a version range pin records nothing and
/// says so (AUP-2), except `off`, which always revokes.
// spec: AUP-2 AUP-6 AUP-8 AUP-9
pub(super) fn apply_auto_update_grant(
    paths: &Paths,
    source_name: &str,
    grant: Option<AutoUpdateGrant>,
    newly_melded: bool,
    yes: bool,
    json: bool,
) -> Result<()> {
    use crate::auto_update::Level;
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) else {
        return Ok(());
    };
    let shown = strip_ansi(&source.name);
    let (declared, ranged) = match &source.pin {
        Pin::Version(vp) => (
            vp.auto_update,
            !matches!(
                vp.parsed_range(),
                None | Some(crate::version::Range::Exact(_))
            ),
        ),
        _ => (None, false),
    };
    let level = match grant {
        Some(AutoUpdateGrant::Declared) => declared.unwrap_or(Level::Compatible),
        Some(AutoUpdateGrant::Level(level)) => level,
        None => {
            let Some(declared) = declared else {
                return Ok(());
            };
            if !newly_melded || !ranged || yes || json || !crate::hook::is_tty() {
                return Ok(());
            }
            let compatible = format!("{shown} asks to update automatically (compatible releases)");
            let asked = if declared == Level::Breaking
                && confirm(&format!(
                    "{shown} asks to update automatically, including breaking releases"
                ))? {
                Some(Level::Breaking)
            } else if confirm_default_yes(&compatible)? {
                Some(Level::Compatible)
            } else {
                None
            };
            let Some(level) = asked else {
                return Ok(());
            };
            level
        }
    };
    if level != Level::Off && !ranged {
        if !json {
            println!(
                "note: auto-update applies only to a version range pin; {shown} has none, so nothing was granted"
            );
        }
        return Ok(());
    }
    source.auto_update = Some(level);
    registry.save(paths)?;
    if !json {
        println!("auto-update: {shown} {level}");
    }
    Ok(())
}

/// `source`'s auto-update level for a run (AUP-1, AUP-2, AUP-10): `off` for
/// anything but a version range pin.
// spec: AUP-2 AUP-10
pub(super) fn auto_update_level(
    source: &crate::source::Source,
    cap: Option<crate::auto_update::Level>,
) -> crate::auto_update::Level {
    use crate::auto_update::Level;
    let Pin::Version(vp) = &source.pin else {
        return Level::Off;
    };
    if matches!(
        vp.parsed_range(),
        None | Some(crate::version::Range::Exact(_))
    ) {
        return Level::Off;
    }
    crate::auto_update::effective(source.auto_update, cap)
}

/// Why moving `source` (already at its new commit in `dir`) would need hook
/// consent a run cannot give (AUP-17): a pending source-level hook, or an
/// installed item whose new content declares an install, update, or build
/// hook. `None` when the move runs no hook.
// spec: AUP-17
fn auto_update_hold_reason(
    paths: &Paths,
    source: &crate::source::Source,
    dir: &std::path::Path,
    manifest: &Manifest,
) -> Result<Option<String>> {
    let pending = pending_update_hooks(source, dir);
    if !pending.hooks.is_empty() {
        return Ok(Some(format!("its {} hook needs consent", pending.event)));
    }
    let mut catalog: Vec<CatalogItem> = Vec::new();
    catalog::scan_source(paths, source, &mut catalog)?;
    for installed in manifest.items.values().filter(|i| i.source == source.name) {
        let Some(cat) = catalog.iter().find(|c| {
            c.kind == installed.kind
                && c.name == installed.bare_name
                && c.source == installed.source
        }) else {
            continue;
        };
        let has_hook = cat.build.is_some()
            || cat.hooks.iter().any(|h| {
                matches!(
                    h.event,
                    crate::mindfile::HookEvent::Install | crate::mindfile::HookEvent::Update
                )
            });
        if has_hook && cat.content_hash()? != installed.hash {
            return Ok(Some(format!(
                "{} has a hook that needs consent",
                installed.display_key()
            )));
        }
    }
    // spec: HOME-16 -- a release whose `mind-home` moves the source would
    // rebuild its built items, and a run cannot consent to their build hooks.
    if !source.chosen_home
        && let Some(declared) = MindToml::load(dir)
            .ok()
            .flatten()
            .and_then(|m| m.source.mind_home)
        && let Ok(target) = paths.allowed_home(&declared, "the `mind-home`")
    {
        use crate::paths::normalize_home;
        let to = normalize_home(target.as_deref().unwrap_or(&paths.mind_home));
        if to != normalize_home(&paths.home_of(&source.name)) {
            let registry = Registry::load(paths)?;
            let family = home_family(paths, &registry, &source.name);
            if let Some(item) = move_would_skip_build(
                paths,
                &registry,
                &family,
                target.as_deref(),
                crate::hook::Accept::default(),
            )? {
                return Ok(Some(format!(
                    "moving it to {} would rebuild {item}, whose build hook needs consent",
                    to.display()
                )));
            }
        }
    }
    Ok(None)
}

/// At `breaking`, move `source` to its recommended range when that range
/// admits its breaking release (AUP-12). Returns the new commit and pin, or
/// `None` when nothing breaking is recommended or the recommended range
/// selects nothing (the source then stays at its compatible resolution).
// spec: AUP-12
fn auto_update_breaking_move(
    source: &crate::source::Source,
    dir: &std::path::Path,
) -> Result<Option<(String, Pin)>> {
    let Pin::Version(vp) = &source.pin else {
        return Ok(None);
    };
    let Some(recommended) = recommended_for_breaking(&source.pin) else {
        return Ok(None);
    };
    let Some(range) = crate::version::Range::parse(&recommended) else {
        return Ok(None);
    };
    let mut moved = source.clone();
    moved.pin = Pin::Version(crate::source::VersionPin {
        range: range.to_string(),
        resolved: None,
        tag: None,
        yanked: false,
        ..vp.clone()
    });
    let (commit, pin) = sync_clone_to_pin(&moved, dir)?;
    match &pin {
        Pin::Version(v) if v.resolved.is_some() => Ok(Some((commit, pin))),
        _ => Ok(None),
    }
}

/// Put `source`'s clone back on the commit the registry records (AUP-17), so a
/// held or failed move leaves the source and its items agreeing.
fn auto_update_restore_clone(source: &crate::source::Source, dir: &std::path::Path) -> Result<()> {
    match source.commit.as_deref() {
        Some(sha) => git::reset_to_commit(&source.url, dir, sha),
        None => git::sync_to_pin(&source.url, dir, &source.pin),
    }
}

/// A version pin's resolved version, for a run report line.
fn resolved_version(pin: &Pin) -> String {
    match pin {
        Pin::Version(v) => v.resolved.clone().unwrap_or_else(|| "?".into()),
        _ => "?".into(),
    }
}

/// `mind auto-update run`'s `--json` result (AUP-13, AUP-21).
#[derive(Serialize)]
struct AutoUpdateJson {
    // spec: CLI-153 CLI-168 -- the mutating-verb envelope.
    pub(super) schema: u8,
    pub(super) action: &'static str,
    pub(super) target: &'static str,
    pub(super) outcome: &'static str,
    pub(super) moved: Vec<String>,
    pub(super) pruned: Vec<String>,
    pub(super) held: Vec<String>,
    pub(super) failed: Vec<String>,
    pub(super) breaking: Vec<BreakingMoveJson>,
}

/// The display keys of the items in `before` whose stable identity (source,
/// kind, bare name) is gone from `after`: what an upgrade pass pruned
/// (LIFE-57). A rename keeps its identity, so it is not counted.
fn removed_identities(before: &Manifest, after: &Manifest) -> Vec<String> {
    let mut keys: Vec<String> = before
        .items
        .values()
        .filter(|b| {
            !after
                .items
                .values()
                .any(|a| a.source == b.source && a.kind == b.kind && a.bare_name == b.bare_name)
        })
        .map(|b| b.display_key())
        .collect();
    keys.sort();
    keys
}

/// `mind auto-update run` (AUP-15..21): sync and upgrade every source whose
/// effective level is not `off`, never prompting.
// spec: AUP-15 AUP-18 AUP-19
pub fn auto_update_run(
    paths: &Paths,
    if_stale: Option<&str>,
    background: bool,
    json: bool,
) -> Result<()> {
    use crate::auto_update::{Level, Report, State};
    let every = if_stale
        .map(crate::auto_update::parse_duration)
        .transpose()?;
    let stale = |state: &State| every.is_none_or(|e| state.is_stale(e, crate::auto_update::now()));
    if background {
        // spec: AUP-20 -- the trigger must never hold a session up: check the
        // throttle here, then hand off to a detached child and return.
        if !stale(&State::load(paths)) {
            return Ok(());
        }
        return spawn_detached_run(if_stale);
    }
    let up_to_date = |json: bool| {
        if json {
            print_json(&AutoUpdateJson {
                schema: 1,
                action: "auto-update run",
                target: "*",
                outcome: "skipped",
                moved: Vec::new(),
                pruned: Vec::new(),
                held: Vec::new(),
                failed: Vec::new(),
                breaking: Vec::new(),
            })
        } else {
            Ok(())
        }
    };
    if !stale(&State::load(paths)) {
        if !json {
            println!("auto-update: the last run is more recent than --if-stale; nothing to do");
        }
        return up_to_date(json);
    }
    // spec: AUP-18 -- never queue behind another mind.
    let mut lock = crate::lock::open(paths)?;
    let Some(_guard) = lock.try_write() else {
        if !json {
            println!("auto-update: another mind command is running; nothing to do");
        }
        return up_to_date(json);
    };
    crate::transaction::recover(paths)?;
    // spec: LIFE-62 -- the unattended run is one transaction like a verb.
    let tx = crate::transaction::begin(paths);
    let result = (|| -> Result<()> {
        // spec: AUP-15 -- every consent gate takes its no-TTY path, and git never
        // asks for credentials.
        crate::hook::force_non_interactive();
        git::set_noninteractive(true);

        let policy = paths.settings().policy()?;
        let manifest = Manifest::load(paths)?;
        let mut registry = Registry::load(paths)?;
        let mut report = Report {
            time: crate::auto_update::now(),
            ..Report::default()
        };
        let mut breaking: Vec<BreakingMoveJson> = Vec::new();
        let mut moved_names: Vec<String> = Vec::new();
        for source in &mut registry.sources {
            let level = auto_update_level(source, auto_update_cap(policy.as_ref(), source));
            if level == Level::Off {
                continue;
            }
            // POL-12: a source the locked allowlist no longer admits is not moved.
            if let Some(p) = policy.as_ref()
                && p.lock()
                && !p.allow_matches(&source.base_identity())
            {
                continue;
            }
            let shown = strip_ansi(&source.name);
            enum Step {
                Unchanged(Box<Pin>),
                Moved(String, Box<Pin>, bool),
                Held(String),
            }
            let current: &crate::source::Source = source;
            let step = (|| -> Result<Step> {
                let dir = clone_dir_checked(paths, current)?;
                let moving = || -> Result<Step> {
                    let (mut commit, mut pin) = sync_clone_to_pin(current, &dir)?;
                    let mut took_breaking = false;
                    if level == Level::Breaking {
                        let mut probe = current.clone();
                        probe.pin = pin.clone();
                        if let Some((c, p)) = auto_update_breaking_move(&probe, &dir)? {
                            commit = c;
                            pin = p;
                            took_breaking = true;
                        }
                    }
                    if current.commit.as_deref() == Some(commit.as_str()) {
                        // Nothing moved; keep the refreshed release metadata.
                        return Ok(Step::Unchanged(Box::new(pin)));
                    }
                    let mut next = current.clone();
                    next.pin = pin.clone();
                    next.commit = Some(commit.clone());
                    if let Some(why) = auto_update_hold_reason(paths, &next, &dir, &manifest)? {
                        // spec: AUP-17 -- put the clone back where the source is
                        // recorded, so the source and its items do not disagree.
                        auto_update_restore_clone(current, &dir)?;
                        return Ok(Step::Held(format!("{}: {why}", resolved_version(&pin))));
                    }
                    // spec: LOBE-2 -- a lobe the new commit names that is refused
                    // fails the move (the hold scan above read it), so the clone
                    // goes back and the run reports it.
                    paths.source_lobe(&next.name)?;
                    Ok(Step::Moved(commit, Box::new(pin), took_breaking))
                };
                let step = moving();
                if step.is_err() {
                    // A failure after the clone moved (a breaking resolution, the
                    // hold scan) must not leave it ahead of the registry.
                    let _ = auto_update_restore_clone(current, &dir);
                }
                step
            })();
            match step {
                Ok(Step::Unchanged(pin)) => source.pin = *pin,
                Ok(Step::Moved(commit, pin, took_breaking)) => {
                    let from = resolved_version(&source.pin);
                    let to = resolved_version(&pin);
                    report.moved.push(format!("{shown} {from} -> {to}"));
                    if took_breaking {
                        // spec: AUP-13
                        breaking.push(BreakingMoveJson {
                            source: shown.clone(),
                            from: from.clone(),
                            to: to.clone(),
                            applied: true,
                            recommended: true,
                        });
                    }
                    source.pin = *pin;
                    source.commit = Some(commit);
                    moved_names.push(source.name.clone());
                }
                Ok(Step::Held(why)) => report.held.push(format!("{shown} {why}")),
                Err(e) => report
                    .failed
                    .push(format!("{shown}: {}", strip_ansi(&e.to_string()))),
            }
        }
        registry.save(paths)?;

        if !moved_names.is_empty() {
            // spec: LIFE-57 AUP-21 -- the pass may prune; record what it removed,
            // since a background run has no one watching.
            let before = Manifest::load(paths)?;
            // The ordinary pass, scoped to what moved, with --yes and no prompts.
            let pass = upgrade_pass(
                paths,
                UpgradeOpts {
                    no_sync: true,
                    sources: SourceScope::Names(&moved_names, None),
                    ..UpgradeOpts::new(true, crate::hook::Accept::default())
                },
            );
            if let Ok(after) = Manifest::load(paths) {
                report.pruned = removed_identities(&before, &after);
            }
            if let Err(e) = pass {
                report
                    .failed
                    .push(format!("upgrade: {}", strip_ansi(&e.to_string())));
            }
        }

        // spec: AUP-19 AUP-21
        crate::auto_update::append_log(paths, &report);
        let mut state = State::load(paths);
        state.last_run = Some(crate::auto_update::now());
        if !report.is_empty() {
            state.unshown.push(report.clone());
        }
        // spec: HOME-1 -- the state lives in the default home, which a sandbox
        // that allows writes only to an extra home cannot write. It throttles and
        // reports; losing it must not fail a run whose updates already applied.
        if let Err(e) = state.save(paths) {
            eprintln!(
                "warning: auto-update state not saved: {}",
                strip_ansi(&e.to_string())
            );
        }

        if json {
            return print_json(&AutoUpdateJson {
                schema: 1,
                action: "auto-update run",
                target: "*",
                outcome: if report.failed.is_empty() {
                    "ok"
                } else {
                    "partial"
                },
                moved: report.moved,
                pruned: report.pruned,
                held: report.held,
                failed: report.failed,
                breaking,
            });
        }
        for line in &report.moved {
            println!("auto-update: moved {line}");
        }
        for line in &report.pruned {
            println!("auto-update: pruned {line}");
        }
        for line in &report.held {
            println!("auto-update: held {line}");
        }
        for line in &report.failed {
            eprintln!("auto-update: failed {line}");
        }
        if report.is_empty() {
            println!("auto-update: everything is up to date");
        }
        Ok(())
    })();
    tx.settle(&result)?;
    result
}

/// Start `mind auto-update run` in a new session with no terminal and return
/// (AUP-20). The child inherits nothing it could print to.
fn spawn_detached_run(if_stale: Option<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let exe = std::env::current_exe().map_err(|e| MindError::io("current_exe", e))?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["auto-update", "run"]);
    if let Some(d) = if_stale {
        cmd.args(["--if-stale", d]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state; it
    // only detaches the child from the parent's session and terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| MindError::io(&exe, e))?;
    Ok(())
}

/// `mind config auto-update on|off` (AUP-20): add or remove the
/// `SessionStart` trigger in each Claude lobe's `settings.json`.
// spec: AUP-20 AUP-30
pub fn config_auto_update(paths: &Paths, on: bool, every: &str) -> Result<()> {
    let out = crate::render::ctx();
    if on {
        crate::auto_update::parse_duration(every)?;
        if paths.settings().policy()?.and_then(|p| p.auto_update_cap())
            == Some(crate::auto_update::Level::Off)
        {
            return Err(MindError::AutoUpdateForbidden);
        }
    }
    let mut changed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for lobe in paths.agent_homes()? {
        let shown = strip_ansi(&lobe.path.display().to_string());
        // A Claude home admits every kind; a harness preset lobe is skill-only.
        if lobe.kinds.is_some() {
            if on {
                skipped.push(shown);
            }
            continue;
        }
        let file = lobe.path.join("settings.json");
        let did = if on {
            crate::auto_update::install_trigger(&file, every)?
        } else {
            crate::auto_update::remove_trigger(&file)?
        };
        if did {
            changed.push(strip_ansi(&file.display().to_string()));
        }
    }
    if out.json {
        let mut r = MutationResult::new(
            "config auto-update",
            if on { "on" } else { "off" },
            if changed.is_empty() { "no-op" } else { "ok" },
        );
        r.count = Some(changed.len());
        return print_json(&r);
    }
    for file in &changed {
        if on {
            println!("{} added the auto-update trigger to {file}", out.ok());
        } else {
            println!("{} removed the auto-update trigger from {file}", out.ok());
        }
    }
    if changed.is_empty() {
        println!(
            "auto-update trigger already {}",
            if on { "installed" } else { "absent" }
        );
    }
    for lobe in &skipped {
        println!(
            "note: {lobe} is not a Claude lobe, so it gets no trigger; schedule `mind auto-update run --if-stale {every}` from cron instead"
        );
    }
    Ok(())
}
