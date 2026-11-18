//! Helpers shared across verbs: result documents, prompts, and output.

use super::*;

/// Thin output-ctx accessor for code that cannot use the `out` binding directly.
pub(super) fn out_ctx() -> crate::render::OutputCtx {
    crate::render::ctx()
}

/// Expand a leading `~` in `path` to the home directory.
pub(crate) fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

/// `mind completions <shell>` — write a shell completion script to stdout.
pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "mind", &mut std::io::stdout());
}

/// `mind man` — write the roff man pages to stdout: the top-level page, then
/// one page per visible subcommand, so the SUBCOMMANDS cross-references
/// (`mind-meld(1)`, ...) resolve within the same output instead of dangling.
// spec: CLI-121
pub fn man() -> Result<()> {
    use clap::CommandFactory;
    let mut out = Vec::new();
    let cmd = crate::cli::Cli::command();
    clap_mangen::Man::new(cmd.clone())
        .render(&mut out)
        .map_err(|e| MindError::io("<man>", e))?;
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        // clap's Str only takes &'static str without its `string` feature;
        // leaking a handful of short page names once per `man` run is fine.
        let name: &'static str = Box::leak(format!("mind-{}", sub.get_name()).into_boxed_str());
        clap_mangen::Man::new(sub.clone().name(name))
            .render(&mut out)
            .map_err(|e| MindError::io("<man>", e))?;
    }
    std::io::stdout()
        .write_all(&out)
        .map_err(|e| MindError::io("<stdout>", e))
}

// --- helpers ---------------------------------------------------------------

/// A source entry that was skipped during meld or sync due to an auth failure
/// with `on-auth-failure.action = "skip"` (DSC-68, DSC-69).
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct SkippedEntry {
    pub(super) source: String,
    pub(super) reason: String,
}

impl SkippedEntry {
    /// Build a skipped entry, sanitizing both string fields (CUR-18).
    ///
    /// `source` is either a curator's free-text entry spec or a source
    /// identity, and an identity is assembled from parts that
    /// `validate_identity_part` screens only for `char::is_control()`: a bidi
    /// override or a zero-width character passes it. `describe()` strips on
    /// the way out, but `--json` serializes these fields directly, so the
    /// sanitizing has to happen where the value is captured -- the same reason
    /// `curate::Change::new` does it in the constructor rather than at each
    /// print site.
    pub(crate) fn new(source: impl Into<String>, reason: impl Into<String>) -> Self {
        SkippedEntry {
            source: crate::sanitize::strip_ansi(&source.into()),
            reason: crate::sanitize::strip_ansi(&reason.into()),
        }
    }

    /// A one-line, ANSI-stripped `source: reason` rendering for text-mode
    /// output (curate.rs's `--json`-free skipped report). The fields are
    /// already sanitized at construction; re-running `strip_ansi` here is
    /// idempotent, not a second pass.
    pub(crate) fn describe(&self) -> String {
        format!(
            "{}: {}",
            crate::sanitize::strip_ansi(&self.source),
            crate::sanitize::strip_ansi(&self.reason)
        )
    }
}

/// Data returned by `meld()` so the dispatcher can combine it with the
/// post-meld install outcome into ONE JSON object (CLI-153, CLI-156).
pub(crate) struct MeldSummary {
    pub(crate) source_name: String,
    pub(crate) added: usize,
    pub(crate) skipped: Vec<SkippedEntry>,
}

/// The structured result a mutating verb emits under `--json` (CLI-153).
/// `action` is the verb, `target` the item/source ref it acted on, and `outcome`
/// a stable token (`installed|removed|melded|synced|upgraded|renamed|no-op|...`).
/// Optional fields are only serialized when a verb genuinely returns more (e.g.
/// `learn` fills `installed` with its closure keys; `sync` fills `count`).
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(super) struct MutationResult {
    // spec: CLI-168 - schema version; always 1.
    pub(super) schema: u8,
    pub(super) action: &'static str,
    pub(super) target: String,
    pub(super) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) installed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) removed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) skipped: Vec<SkippedEntry>,
    /// Count of items available to install but not yet installed (no `--yes` given).
    /// Only set for `meld` JSON results when items are pending (CLI-156).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pending_items: Option<usize>,
    /// The managed `kind:name` key after a successful absorb (ABS-11).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) key: Option<String>,
    /// The enabled install profiles (PROF-18), set by a `meld`/`learn` that
    /// went through a profile install.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) profiles: Option<Vec<String>>,
    /// Items newly in an enabled profile that `upgrade` proposed (PROF-21).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) profile_additions: Vec<ProfileAddition>,
    /// Snapshot keys that left their profile, reported by `upgrade`
    /// (PROF-22).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) profile_departures: Vec<ProfileDeparture>,
    /// Items new to a source installed whole that `upgrade` proposed
    /// (LIFE-54, LIFE-56).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) new_items: Vec<NewItem>,
    /// New items `upgrade --ignore-new` recorded as seen without installing
    /// (LIFE-55, LIFE-56).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) ignored_new_items: Vec<NewItem>,
    /// Items `forget <source> --profile` kept because another installed item
    /// depends on them (PROF-25, DEP-60), each naming its dependents.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) kept: Vec<String>,
    /// The profile leftovers (PROF-37) a re-meld that replaced the enabled set
    /// left installed for `prune` (PROF-40).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) profile_leftovers: Vec<String>,
    /// Each version-pinned source a `sync` or `upgrade` covered, with its
    /// release state (VER-91).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) sources: Vec<SourceVersionJson>,
    /// The breaking releases an `upgrade` listed or took (VER-91).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) breaking: Vec<BreakingMoveJson>,
}

impl MutationResult {
    /// A result with no optional fields populated.
    pub(super) fn new(action: &'static str, target: &str, outcome: &'static str) -> Self {
        Self {
            schema: 1,
            action,
            target: target.to_string(),
            outcome,
            count: None,
            installed: Vec::new(),
            removed: Vec::new(),
            skipped: Vec::new(),
            pending_items: None,
            key: None,
            profiles: None,
            profile_additions: Vec::new(),
            profile_departures: Vec::new(),
            new_items: Vec::new(),
            ignored_new_items: Vec::new(),
            kept: Vec::new(),
            profile_leftovers: Vec::new(),
            sources: Vec::new(),
            breaking: Vec::new(),
        }
    }
}

/// True when the current output context wants machine JSON. A mutating verb gates
/// its prose on `!json_mode()` and emits a [`MutationResult`] when it is true.
pub(super) fn json_mode() -> bool {
    crate::render::ctx().json
}

/// Emit a verb's JSON result (CLI-153).
///
/// When stdout is reserved (`--json` on a verb that answers with a document,
/// CLI-217) the document is RECORDED rather than printed: `main` writes the
/// last one recorded, once, to the preserved stdout. Every `print_json` call in
/// this module goes through here, so a nested verb invoked on another verb's
/// behalf (`meld`/`sync` -> `learn`) can no longer put a second document on
/// stdout, whatever the call site does. Outside that mode it is
/// `render::print_json` unchanged.
// spec: CLI-217
pub(crate) fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value).map_err(|e| MindError::json("json output", e))?;
    if crate::json_stdout::is_reserved() {
        crate::json_stdout::record(s);
        return Ok(());
    }
    // spec: HOOK-161 -- unreserved (no redirect on this platform), the
    // document is written here, so the hook results are added here too.
    println!("{}", crate::json_stdout::with_hooks(s));
    Ok(())
}

/// [`print_json`] for the `{"schema": 1, "items": [...]}` array envelope
/// (CLI-167), with the same CLI-217 recording behavior.
// spec: CLI-217
pub(super) fn print_json_envelope<T: Serialize>(items: &[T]) -> Result<()> {
    if crate::json_stdout::is_reserved() {
        // Same envelope `render::print_json_envelope` builds, routed through the
        // recording emitter above instead of straight to stdout.
        #[derive(Serialize)]
        struct Envelope<'a, T: Serialize> {
            schema: u8,
            items: &'a [T],
        }
        return print_json(&Envelope { schema: 1, items });
    }
    crate::render::print_json_envelope(items)
}

/// A throwaway registry holding just one source, for catalog scans during meld.
pub(crate) fn single(source: &crate::source::Source) -> Registry {
    Registry {
        sources: vec![source.clone()],
    }
}

/// DSC-93: the resolved path of a `[discover].sources` entry that names a LOCAL
/// path which (a) does not exist and (b) lies inside mind's managed sources tree
/// (`<mind_home>/sources`), i.e. a relative curator entry DSC-92 resolved against
/// a cloned copy of that curator. `None` for every other entry: a remote spec, an
/// absolute local path outside the sources tree, or any path that actually
/// exists (including a sibling clone the same walk already created, which the
/// DSC-70 already-registered guard handles).
///
/// The predicate is deliberately structural rather than name-based: mind is the
/// only writer of the sources tree, and it never creates the sibling a curator's
/// `../x` would need, so a non-existent path there can only be a misresolved
/// relative reference -- no matching of the entry against other entries in the
/// walk is required, and identity (`host/owner/repo`), which the two readings
/// disagree on, is never consulted.
// spec: DSC-93
pub(super) fn unresolvable_managed_local_entry(
    paths: &Paths,
    spec: &str,
) -> Option<std::path::PathBuf> {
    // The quiet parse: this is a classification, not a decision to clone, so the
    // CLI-215 shadowing note would be noise here (CLI-216).
    let parsed = crate::source::parse_spec_quiet(spec).ok()?;
    if parsed.host != "local" {
        return None;
    }
    let path = std::path::PathBuf::from(&parsed.url);
    (!path.exists()
        && paths
            .all_homes()
            .iter()
            .any(|h| path.starts_with(h.join("sources"))))
    .then_some(path)
}

/// Scan exactly ONE source, hard-failing on any scan error.
///
/// The counterpart to `catalog::scan(paths, &single(src))`, which routes through
/// the whole-registry walk and therefore inherits its CLI-213 degradation: a
/// `LinkedSourceGone` source is skipped with a stderr warning and the call
/// returns `Ok(vec![])`. That degradation exists so one dead source does not take
/// down a LISTING of every source; it is wrong for a caller that named a single
/// source and needs to know whether THAT source could be read, because an empty
/// result is indistinguishable from a healthy source with no items.
// spec: CLI-212 CLI-213
pub(super) fn scan_one(paths: &Paths, source: &crate::source::Source) -> Result<Vec<CatalogItem>> {
    let mut items = Vec::new();
    catalog::scan_source(paths, source, &mut items)?;
    Ok(items)
}

/// Report a manifest-save failure that occurred while already unwinding from
/// another error (LIFE-48's double-failure outcome, CLI-232): the root cause
/// `e` is about to propagate, and persisting the manifest to record what
/// already happened on disk ALSO failed, so on-disk state may now not match
/// `manifest.json`.
///
/// Routed through `render::warn` rather than a bare `eprintln!` so the
/// message is sanitized (DSC-95 -- `se`'s Display can embed a source-
/// controlled path) and follows the same stdout/text vs stderr/json routing
/// as every other advisory line (CLI-217). It also drops the `error: `
/// prefix the bare `eprintln!` reused: `main.rs` already prints `error: {err}`
/// for the propagated root cause, so two lines both starting `error: ` read
/// as two candidates for "the" error rather than a root cause plus a
/// secondary complication. Names `mind introspect --fix` as the remedy.
// spec: CLI-232 LIFE-48
pub(super) fn warn_manifest_save_also_failed(se: &MindError) {
    crate::render::warn(format!(
        "also failed to persist the manifest: {}; on-disk state may not match \
         manifest.json -- run `mind introspect --fix` to reconcile",
        strip_ansi(&se.to_string())
    ));
}

/// Build the `action` text for a `ConfirmationRequired` raised because
/// `--json` is non-interactive (LIFE-45), as opposed to stdin merely not
/// being a TTY. The variant's fixed suffix ("re-run with --yes (or in an
/// interactive terminal)") is misleading for a `--json` caller: retrying in
/// a real terminal changes nothing, because `--json` disables interactive
/// confirmation unconditionally regardless of the TTY. Route through this so
/// the action text names the only remedy that actually applies.
// spec: CLI-232
pub(super) fn json_confirmation_action(base: impl std::fmt::Display, json: bool) -> String {
    if json {
        format!("{base} (--json is always non-interactive; --yes is the only way to proceed)")
    } else {
        base.to_string()
    }
}

/// Build the note naming an upgrade pass's matched sources when the scope
/// spans more than one source (CLI-233; extended from `sync <filter>
/// --upgrade`-only to `upgrade <item>` too by CLI-234). `names` is the union
/// of every scoping mechanism in play, computed once by the caller
/// (`upgrade_inner_scoped`'s `scope_names`); a filter (unlike `unmeld`'s/
/// `upgrade`'s ref-style SOURCE selection, CLI-20) intentionally allows more
/// than one match, so a name like `mind sync skills --upgrade` or `mind
/// upgrade 'suffix#*'` can silently widen to upgrading -- and re-running the
/// install hook of (HOOK-11) -- every source the filter happened to match,
/// while reading as if it named one.
///
/// `filter_desc`, when given, is the raw filter TEXT that produced the match
/// (the `sync` selector string or the `upgrade` item-ref string, M19),
/// echoed so the disclosure names what did the matching, not just how many
/// sources it hit.
///
/// spec: CLI-235 -- this now fires BEFORE `pending` is computed (hoisted
/// above `rerun_source_hooks`, M1), so it can no longer claim to name only
/// the sources that end up contributing a pending upgrade: it says plainly
/// that every matched source is in scope for this pass, present tense (not
/// "possibly re-running", which read as a future warning for a hook re-run
/// that, before the M1 fix, had already happened by the time this printed).
///
/// `None` when `names` has zero or one entries: an unscoped `--upgrade` pass
/// (bare `mind upgrade`, or `sync`/`upgrade` with no filter) and a
/// single-match filter both leave the confirmation exactly as it was before
/// CLI-233 -- a single match already reads as naming that one source, so
/// growing the disclosure there would be noise, not signal.
///
/// `filters` is the text of every filter that shaped the scope: none for the
/// TUI's key-scoped apply, one for `sync <filter>` or `upgrade <item>`, two for
/// `upgrade <item> --source <filter>` (CLI-244), whose scope is their
/// intersection, so quoting only one would misstate what did the matching.
// spec: CLI-233 CLI-234 CLI-235 CLI-244
pub(super) fn multi_source_upgrade_note(names: &[String], filters: &[&str]) -> Option<String> {
    if names.len() <= 1 {
        return None;
    }
    let mut sanitized: Vec<String> = names.iter().map(|n| strip_ansi(n)).collect();
    sanitized.sort();
    // spec: M19 -- end with the narrowing remedy (a longer suffix, or the
    // full `host/owner/repo` identity), matching every other confirmation in
    // this module that states a real next step rather than just a warning.
    let quoted: Vec<String> = filters
        .iter()
        .map(|f| format!("\"{}\"", strip_ansi(f)))
        .collect();
    let scope = match quoted.as_slice() {
        [] => "this upgrade's scope".to_string(),
        [one] => format!("the filter {one}"),
        many => format!("the filters {} together", many.join(" and ")),
    };
    Some(format!(
        "{scope} matched {} sources; every matched source's items are in scope for this pass, and its install hook may be re-run: {}. Narrow with a longer suffix, or the full host/owner/repo identity, to target fewer.",
        sanitized.len(),
        sanitized.join(", "),
    ))
}

/// First 8 chars of a hash/commit, for compact display.
pub(super) fn short(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.chars().take(8).collect()
    }
}

/// One-line summary for list views: first sentence or `max` chars, whichever is
/// shorter. The full text stays available via `recall <item>`.
pub(super) fn summary(desc: Option<&str>, max: usize) -> String {
    let Some(d) = desc else { return String::new() };
    let first = d.split(". ").next().unwrap_or(d).trim_end_matches('.');
    if first.chars().count() <= max {
        return first.to_string();
    }
    let cut: String = first.chars().take(max.saturating_sub(1)).collect();
    format!("{}...", cut.trim_end())
}

/// Print `prompt` and read one line from stdin (trimmed by the caller). EOF
/// yields an empty string. Used for free-form answers like the prefix prompt
/// (CLI-24), where `[y/N]` is not enough.
pub(super) fn prompt_line(prompt: &str) -> Result<String> {
    // spec: DSC-95 -- a prompt built from a source-controlled name (the
    // meld prefix preview, CLI-24) can carry ANSI/control/bidi bytes; sanitize
    // before it reaches the terminal.
    let prompt = crate::sanitize::strip_ansi(prompt);
    // spec: TUI-44
    crate::prompt::guard("answering a prompt")?;
    // spec: PRG-5
    crate::progress::suspend(|| {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| MindError::io("<stdin>", e))?
            == 0
        {
            return Ok(String::new()); // EOF -> empty (accept the declared prefix)
        }
        Ok(line)
    })
}

/// Ask `prompt {hint}` as a yes/no question (CLI-257..259): an unrecognized
/// reply is asked again, Enter and EOF take `default_yes`, and without a TTY
/// the run refuses with `ConfirmationRequired` rather than prompting.
pub(super) fn read_confirm(prompt: &str, hint: &str, default_yes: bool) -> Result<bool> {
    // spec: DSC-95 -- the single chokepoint every `confirm`/
    // `confirm_default_yes` call routes through; sanitize here so a
    // source-controlled name baked into a prompt string cannot carry ANSI/
    // control/bidi bytes to the terminal.
    let prompt = crate::sanitize::strip_ansi(prompt);
    // spec: CLI-259 -- the refusal names the question that was not asked.
    let action = prompt.trim_end_matches('?').to_string();
    crate::prompt::ask(
        &format!("\n{prompt} {hint} "),
        &crate::prompt::YES_NO,
        default_yes,
        &action,
    )
}

// ----- Unified install plan (spec/install-hooks.md, HOOK-180..186) -----

/// What a plan's answer leaves the caller to do (HOOK-181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanOutcome {
    /// Go ahead with the items; the listed hooks run, skip, or are reviewed as
    /// answered.
    Proceed,
    /// `n`: install (or remove) nothing.
    Cancel,
    /// A deferred required source hook was aborted under `r` (HOOK-21): the
    /// meld already stopped and said so.
    Abort,
}

/// The item half of a plan's question (HOOK-180).
pub(crate) struct PlanItems<'a> {
    /// The question, e.g. `install these 3 item(s) now?`.
    pub question: &'a str,
    /// Ask it even when no hook needs an answer: the verb had its own item
    /// confirmation (CLI-23, CLI-42, CLI-60, DEP-31). Without it, a run with no
    /// hook to ask about asks nothing (HOOK-180).
    pub always: bool,
    /// The default of the item-only `[Y/n]` / `[y/N]` form.
    pub default_yes: bool,
    /// A removal (`unmeld`, HOOK-186): the four-way prompt defaults to `n`
    /// (`[y/s/r/N]`) instead of `y`.
    pub removal: bool,
    /// Lines listing the items, printed before the plan when the caller has
    /// not already shown them.
    pub listing: Vec<String>,
}

/// The replies of the four-way plan prompt (HOOK-181, CLI-257).
const PLAN_CHOICES: [crate::prompt::Choice<(PlanOutcome, crate::consent::Answer)>; 4] = [
    crate::prompt::Choice {
        keys: &["y", "yes"],
        value: (PlanOutcome::Proceed, crate::consent::Answer::RunAll),
    },
    crate::prompt::Choice {
        keys: &["s", "skip"],
        value: (PlanOutcome::Proceed, crate::consent::Answer::SkipAll),
    },
    crate::prompt::Choice {
        keys: &["r", "review"],
        value: (PlanOutcome::Proceed, crate::consent::Answer::Review),
    },
    crate::prompt::Choice {
        keys: &["n", "no"],
        value: (PlanOutcome::Cancel, crate::consent::Answer::SkipAll),
    },
];

/// The replies of the hooks-only prompt (HOOK-182, CLI-257).
const PLAN_HOOK_CHOICES: [crate::prompt::Choice<crate::consent::Answer>; 3] = [
    crate::prompt::Choice {
        keys: &["y", "yes"],
        value: crate::consent::Answer::RunAll,
    },
    crate::prompt::Choice {
        keys: &["n", "no"],
        value: crate::consent::Answer::SkipAll,
    },
    crate::prompt::Choice {
        keys: &["r", "review"],
        value: crate::consent::Answer::Review,
    },
];

/// Show one plan and ask once (HOOK-180..183): the items (when `items` is
/// given; `None` when `--yes` answered them or there are none), then every
/// hook the run would offer, deferred meld hooks first. The answer is recorded
/// against each listed hook (`consent`), and any deferred meld hooks are then
/// offered under it, before the caller installs anything (HOOK-87).
///
/// Hooks a previous plan in the same run already answered are not listed
/// again. Off a TTY, or under `--json`, nothing is asked here: those runs keep
/// their own paths (HOOK-22, CLI-23, CLI-259) and the item question, when
/// `always`, refuses through `read_confirm`.
pub(crate) fn ask_plan(
    paths: &Paths,
    items: Option<PlanItems<'_>>,
    hooks: Vec<crate::consent::PlannedHook>,
) -> Result<PlanOutcome> {
    let interactive = crate::hook::is_tty() && !json_mode();
    let mut listed = deferred_meld_plan(paths)?;
    listed.extend(hooks);
    listed.retain(|h| crate::consent::answer_for(&h.key).is_none());
    let asking = interactive && listed.iter().any(|h| !h.accepted);

    let (outcome, answer) = if asking {
        // spec: HOOK-180 HOOK-181 HOOK-182
        crate::progress::suspend(|| {
            if let Some(it) = &items {
                for line in &it.listing {
                    println!("{line}");
                }
            }
            print!("{}", crate::consent::render(&listed));
        });
        let prompting = listed.iter().filter(|h| !h.accepted).count();
        match &items {
            Some(it) => {
                // spec: HOOK-181 HOOK-186 -- an install defaults to yes; a
                // removal defaults to no.
                let (yes_key, no_key, hint, default) = if it.removal {
                    ("y", "N", "[y/s/r/N]", PLAN_CHOICES[3].value)
                } else {
                    ("Y", "n", "[Y/s/r/n]", PLAN_CHOICES[0].value)
                };
                println!(
                    "  [{yes_key}] yes, run the hooks   [s] yes, skip the hooks   [r] yes, ask about each hook   [{no_key}] no"
                );
                let (outcome, answer) = crate::prompt::ask(
                    &format!(
                        "{} ({prompting} hook(s) to run) {hint} ",
                        strip_ansi(it.question)
                    ),
                    &PLAN_CHOICES,
                    default,
                    it.question.trim_end_matches('?'),
                )?;
                (outcome, Some(answer))
            }
            None => {
                println!("  [Y] run them   [n] skip them   [r] ask about each hook");
                let answer = crate::prompt::ask(
                    &format!("run these {prompting} hook(s)? [Y/n/r] "),
                    &PLAN_HOOK_CHOICES,
                    crate::consent::Answer::RunAll,
                    "running the listed hooks",
                )?;
                (PlanOutcome::Proceed, Some(answer))
            }
        }
    } else if let Some(it) = items.filter(|it| it.always) {
        // spec: HOOK-182 -- every hook is accepted (or there are none): the
        // item confirmation alone, with the accepted hooks shown.
        if interactive && !listed.is_empty() {
            for line in &it.listing {
                println!("{line}");
            }
            print!("{}", crate::consent::render(&listed));
        }
        let yes = if it.default_yes {
            confirm_default_yes(it.question)?
        } else {
            confirm(it.question)?
        };
        let outcome = if yes {
            PlanOutcome::Proceed
        } else {
            PlanOutcome::Cancel
        };
        (outcome, None)
    } else {
        (PlanOutcome::Proceed, None)
    };

    if let Some(answer) = answer {
        crate::consent::grant(listed.iter().map(|h| &h.key), answer);
    }
    // spec: HOOK-183 -- the deferred meld hooks run now, before any item: under
    // the answer (a cancel skips and records them, so a re-meld re-offers them,
    // HOOK-60).
    if outcome == PlanOutcome::Cancel {
        let deferred = deferred_meld_plan(paths)?;
        crate::consent::grant(
            deferred.iter().map(|h| &h.key),
            crate::consent::Answer::SkipAll,
        );
    }
    if matches!(run_deferred_meld_hooks(paths)?, HookOutcome::Abort) {
        return Ok(PlanOutcome::Abort);
    }
    Ok(outcome)
}

/// The plan entries for installing `item` (HOOK-180, HOOK-184): its build hook
/// (HOOK-70) and its install hooks, or its update hooks on a re-install
/// (HOOK-125), in run order. Each names the directory it runs in and the
/// item's clone path to inspect, since the run directory does not hold the
/// item yet.
pub(super) fn item_install_plan(
    paths: &Paths,
    registry: &Registry,
    item: &CatalogItem,
    is_update: bool,
    accept: crate::hook::Accept,
) -> Vec<crate::consent::PlannedHook> {
    let commit = registry
        .find(&item.source)
        .and_then(|s| s.commit.clone())
        .unwrap_or_default();
    let name = item.effective_name();
    let display_key = item.display_key();
    let inspect = Some(item.path.display().to_string());
    let mut plan = Vec::new();
    if let Some(build) = &item.build {
        plan.push(crate::consent::PlannedHook {
            key: crate::consent::HookKey::item_hook(&item.source, &display_key, "build", build),
            label: "build".to_string(),
            // A build hook offers run or skip, never abort (HOOK-72).
            optional: true,
            commit: commit.clone(),
            run_dir: paths
                .staging_path_of(&item.source, item.kind, &name)
                .display()
                .to_string(),
            inspect: inspect.clone(),
            browse_url: None,
            replaced: None,
            accepted: accept.build,
        });
    }
    // The install hook's working directory (HOOK-81): the store directory of a
    // directory-backed item, else the kind directory holding its file.
    let store = paths.store_item_of(&item.source, item.kind, &name);
    let run_dir = if item.path.is_dir() {
        store
    } else {
        store
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or(store)
    };
    for h in item.install_time_hooks(is_update) {
        plan.push(crate::consent::PlannedHook {
            key: crate::consent::HookKey::item_hook(
                &item.source,
                &display_key,
                h.event.as_str(),
                &h.run,
            ),
            label: h.label().to_string(),
            optional: h.optional,
            commit: commit.clone(),
            run_dir: run_dir.display().to_string(),
            inspect: inspect.clone(),
            browse_url: None,
            replaced: None,
            accepted: accept.event(h.event),
        });
    }
    plan
}

/// The plan entries for removing an installed item (HOOK-186): its uninstall
/// hooks (HOOK-82), run in its store copy, which exists, so there is nothing
/// separate to inspect. Empty when the store copy is gone, since
/// [`uninstall_item`] then runs no hook.
pub(super) fn item_uninstall_plan(
    paths: &Paths,
    item: &crate::manifest::InstalledItem,
    hooks: &[&crate::mindfile::ResolvedHook],
    commit: &str,
    accept: crate::hook::Accept,
) -> Vec<crate::consent::PlannedHook> {
    let store = paths.mind_home.join(&item.store);
    if !store.exists() {
        return Vec::new();
    }
    let run_dir = if store.is_dir() {
        store
    } else {
        store
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or(store)
    };
    let display_key = item.display_key();
    hooks
        .iter()
        .map(|h| crate::consent::PlannedHook {
            key: crate::consent::HookKey::item_hook(
                &item.source,
                &display_key,
                "uninstall",
                &h.run,
            ),
            label: h.label().to_string(),
            optional: h.optional,
            commit: commit.to_string(),
            run_dir: run_dir.display().to_string(),
            inspect: None,
            browse_url: None,
            replaced: None,
            accepted: accept.uninstall,
        })
        .collect()
}

/// Prompt `[y/N]` on the terminal; default No.
pub(crate) fn confirm(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[y/N]", false)
}

/// Like `confirm` but defaulting to yes (`[Y/n]`): a bare Enter (or EOF)
/// confirms. Used where the affirmative is the expected
/// path and the action is reversible (the meld install-items prompt, CLI-23).
pub(crate) fn confirm_default_yes(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[Y/n]", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    // CLI-153: the mutating-verb JSON result has the stable
    // action/target/outcome shape; optional fields appear only when populated.
    #[test]
    fn mutation_result_minimal_shape() {
        let r = MutationResult::new("forget", "skill:review", "removed");
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["action"], "forget");
        assert_eq!(v["target"], "skill:review");
        assert_eq!(v["outcome"], "removed");
        // Unpopulated optional fields are omitted entirely.
        assert!(v.get("count").is_none(), "count must be omitted: {v}");
        assert!(v.get("installed").is_none(), "installed omitted: {v}");
        assert!(v.get("removed").is_none(), "removed omitted: {v}");
    }

    #[test]
    fn mutation_result_populated_optional_fields() {
        let mut r = MutationResult::new("learn", "skill:review", "installed");
        r.installed = vec!["agent:reviewer".to_string(), "skill:review".to_string()];
        r.count = Some(2);
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["action"], "learn");
        assert_eq!(v["outcome"], "installed");
        assert_eq!(v["count"], 2);
        assert_eq!(
            v["installed"],
            serde_json::json!(["agent:reviewer", "skill:review"])
        );
        // `removed` is still empty, so it is omitted.
        assert!(v.get("removed").is_none(), "empty removed omitted: {v}");
    }

    /// expand_tilde expands a leading `~` to the home directory.
    // spec: ABS-2 ABS-3
    #[test]
    fn expand_tilde_handles_home_prefix() {
        let home = dirs::home_dir().expect("home dir");
        // Bare `~` expands to home.
        let expanded = expand_tilde("~");
        assert_eq!(expanded, home, "bare ~ must expand to home directory");
        // `~/foo` expands to home/foo.
        let expanded2 = expand_tilde("~/foo");
        assert_eq!(
            expanded2,
            home.join("foo"),
            "~/foo must expand to <home>/foo"
        );
        // An absolute path passes through unchanged.
        let abs = expand_tilde("/tmp/mydir");
        assert_eq!(
            abs,
            PathBuf::from("/tmp/mydir"),
            "absolute path must be unchanged"
        );
        // A relative path (no tilde) passes through unchanged.
        let rel = expand_tilde("relpath/dir");
        assert_eq!(
            rel,
            PathBuf::from("relpath/dir"),
            "relative path must be unchanged"
        );
    }

    /// `scan_one` is the non-degrading counterpart of
    /// `catalog::scan(paths, &single(src))`, and the difference between the two
    /// is the whole point of every site that moved onto it. The DSC-80 curator
    /// guard in `meld_recursive` is the one such site that cannot be driven into
    /// this state from the CLI hermetically (it scans the primary source it just
    /// registered, whose directory was verified moments earlier), so the contract
    /// it now rests on is pinned here directly: for the SAME gone source, the
    /// registry walk yields an empty catalog and the single-source scan errors.
    /// Were the guard still on the degrading scan, a `LinkedSourceGone` primary
    /// would scan as empty and be reported as `CuratorAllNestedFailed` -- a real
    /// condition, but not this one.
    #[test]
    fn scan_one_hard_fails_where_the_registry_walk_degrades() {
        // spec: CLI-212 CLI-213
        let (paths, base) = ns_paths();
        let gone = base.join("never-created");
        let src = crate::source::parse_spec(&gone.to_string_lossy()).expect("parse local spec");
        assert!(
            src.is_linked(),
            "sanity: a local unpinned source is linked (CLI-27), which is what \
             LinkedSourceGone applies to"
        );

        let degraded =
            catalog::scan(&paths, &single(&src)).expect("the registry walk degrades, CLI-213");
        assert!(
            degraded.is_empty(),
            "the degraded result is indistinguishable from a healthy source with \
             no items, which is exactly why a single-source caller cannot use it"
        );

        let err = scan_one(&paths, &src).expect_err("a single-source scan must not degrade");
        assert!(
            matches!(err, MindError::LinkedSourceGone { .. }),
            "the vanished working tree must surface as LinkedSourceGone (with its \
             `mind unmeld` remedy), got: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- DSC-93: the absolute entry wins ----

    /// The DSC-93 predicate, at its four boundaries. It is structural (a
    /// non-existent path inside mind's managed sources tree) rather than
    /// name-based, so it must not fire for a remote entry, for a mistyped
    /// relative path in a LINKED curator (which resolves into the user's own
    /// tree and still fails as DSC-79/DSC-80 describe), or for a path that
    /// actually exists (a sibling clone the same walk already created, which the
    /// DSC-70 already-registered guard handles).
    #[test]
    fn dsc93_skips_only_a_missing_path_inside_the_managed_sources_tree() {
        // spec: DSC-93
        let (paths, base) = ns_paths();

        // (1) A remote entry: never a local path, never skipped.
        assert_eq!(
            unresolvable_managed_local_entry(&paths, "github:acme/nested"),
            None,
            "a remote entry must never be skipped"
        );

        // (2) A local path outside the sources tree that does not exist: a
        // curator typo in the user's own working tree, which keeps failing.
        let typo = base.join("curator-worktree").join("nested-lib");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &typo.to_string_lossy()),
            None,
            "a missing path outside the sources tree is a real error, not a skip"
        );

        // (3) A path inside the sources tree that does not exist: the misresolved
        // relative reference of a cloned curator.
        let phantom = paths.sources_dir().join("local/owner/nested-lib");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            Some(phantom.clone()),
            "a missing sibling inside mind's own sources tree is the DSC-93 case"
        );

        // (4) The same path, once something IS cloned there: attempted as usual,
        // so DSC-70's already-registered guard stays in charge of it.
        std::fs::create_dir_all(&phantom).expect("create the sibling clone dir");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            None,
            "an entry that resolves to a directory that exists must be attempted"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The DSC-93 predicate reaches the sources tree through
    /// `parse_spec_quiet`, which has more local-path branches than the plain
    /// absolute path the test above uses. Each of them can appear verbatim in a
    /// curator's `[discover].sources`, so each must classify the same way; a
    /// branch the predicate silently misses is a curator entry that goes back
    /// to being attempted, failing to clone, and (DSC-80) aborting a whole
    /// `dump` reproduction.
    #[test]
    fn dsc93_classifies_every_local_spec_form_the_parser_accepts() {
        // spec: DSC-93 CLI-216
        let (paths, base) = ns_paths();
        let phantom = paths.sources_dir().join("local/owner/nested-lib");

        // The `file://` spelling of the same missing path. `parse_spec` treats
        // it as the identical local repo, so the predicate must too.
        let file_url = format!("file://{}", phantom.display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &file_url),
            Some(phantom.clone()),
            "the file:// spelling of a missing sources-tree path is the same case \
             as the bare path"
        );

        // A local ITEM LINK (LNK-1) inside the sources tree: the predicate must
        // classify by the REPO part (what would be cloned), not by the deep
        // path, which never exists as a directory in its own right.
        let link = format!("file://{}/tree/main/skills/greet", phantom.display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &link),
            Some(phantom.clone()),
            "a local item-link entry must be classified by its repo part"
        );

        // A relative spec is resolved against the CWD, never against the
        // sources tree, so it can never be the DSC-93 case. (DSC-92 has already
        // rewritten a curator's relative entry to an absolute path by the time
        // the predicate sees it; this pins that a leftover relative string does
        // not accidentally match.)
        assert_eq!(
            unresolvable_managed_local_entry(&paths, "../nested-lib"),
            None,
            "a relative entry resolves against the cwd, never into the sources tree"
        );

        // A sibling whose name merely SHARES A PREFIX with the sources dir is
        // outside it: `starts_with` is component-wise, and this pins that.
        let neighbour = format!("{}-elsewhere/owner/nested", paths.sources_dir().display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &neighbour),
            None,
            "a path whose string merely shares a prefix with the sources dir is \
             not inside it"
        );

        // Something exists at the path, but as a FILE. `exists()` is true, so
        // the entry is attempted and fails as an ordinary nested clone failure
        // rather than being skipped. Documented, not incidental: mind never
        // writes a file there, so this cannot arise from mind's own bookkeeping.
        std::fs::create_dir_all(phantom.parent().expect("parent")).expect("mkdir");
        std::fs::write(&phantom, b"not a clone").expect("write file at the clone path");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            None,
            "a path occupied by a FILE exists, so it is attempted (and fails as a \
             clone error) rather than skipped"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- NS-44: collision prompt parsing (parse_collision_answer) ----

    #[test]
    fn multi_source_upgrade_note_none_for_empty_scope() {
        // spec: CLI-233 -- an unscoped `--upgrade` pass (bare `mind upgrade`,
        // or `sync --upgrade` with no `[source]` filter) must not grow the
        // prompt: `scope_names` is empty in that case.
        assert!(multi_source_upgrade_note(&[], &[]).is_none());
    }

    #[test]
    fn multi_source_upgrade_note_none_for_single_match() {
        // spec: CLI-233 -- a filter that resolved to exactly one source reads
        // like naming that source already; the prompt must not grow.
        let names = vec!["github.com/acme/skills".to_string()];
        assert!(
            multi_source_upgrade_note(&names, &["skills"]).is_none(),
            "a single-match filter must not grow the confirmation"
        );
    }

    #[test]
    fn multi_source_upgrade_note_names_every_match_for_multi_match() {
        // spec: CLI-233 -- the defect this covers: `mind sync skills
        // --upgrade` can match several sources while reading as if it named
        // one. When the filter matches more than one source, the note must
        // name every one of them so the blast radius (each matched source's
        // items upgraded, and its install hook possibly re-run) is visible
        // before consent.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &["skills"])
            .expect("a multi-match filter must produce a naming note");
        assert!(
            note.contains("github.com/acme/skills"),
            "note must name the first matched source: {note}"
        );
        assert!(
            note.contains("github.com/other/skills"),
            "note must name the second matched source: {note}"
        );
        assert!(
            note.contains('2'),
            "note must state the match count: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_echoes_the_filter_string() {
        // spec: CLI-235 -- M19: the note must echo the filter TEXT that did
        // the matching, not just the count/names of what it matched.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &["skills"]).expect("multi-match note");
        assert!(
            note.contains("skills"),
            "note must echo the filter string: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_quotes_both_filters_when_combined() {
        // spec: CLI-244 -- `upgrade <item> --source <filter>`'s scope is the
        // intersection of both, so the note quotes both, not the item alone.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note =
            multi_source_upgrade_note(&names, &["skills#*", "*/skills"]).expect("multi-match note");
        assert!(
            note.contains("\"skills#*\"") && note.contains("\"*/skills\""),
            "note must quote both filters: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_names_a_narrowing_remedy() {
        // spec: CLI-235 -- M19: the note must end with a real next step (a
        // longer suffix, or the full `host/owner/repo` identity), matching
        // every other confirmation in this module.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &["skills"]).expect("multi-match note");
        assert!(
            note.contains("host/owner/repo") || note.to_lowercase().contains("narrow"),
            "note must state a narrowing remedy: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_omits_filter_clause_when_no_filter_text() {
        // spec: CLI-234 -- the TUI's key-scoped apply has no textual filter
        // to echo (`filter_desc = None`); the note must still fire (the union
        // scope can still span more than one source) without claiming to
        // quote a filter that was never given.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &[]).expect("multi-match note");
        assert!(
            !note.contains('"'),
            "must not fabricate a quoted filter string: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_strips_ansi_from_source_names() {
        // spec: CLI-233 DSC-95 -- a source name is source-controlled (an
        // `[source].description`-adjacent identity string can be influenced by
        // the melded repo); sanitize each name before composing the note,
        // mirroring every other DSC-95 print site in this module.
        let names = vec![
            "\x1b[2Jevil".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &[]).expect("multi-match note");
        assert!(
            !note.contains('\x1b'),
            "ANSI escape byte must be stripped: {note:?}"
        );
        assert!(note.contains("evil"), "printable text must survive: {note}");
    }

    #[test]
    fn json_confirmation_action_embeds_multi_source_note() {
        // spec: CLI-233 -- under `--json`, `sync <filter> --upgrade`'s
        // `ConfirmationRequired` (LIFE-45: --json never prompts) still names
        // the matched sources in its `action` text, so a machine caller sees
        // the same blast-radius disclosure a human would at the text prompt,
        // rather than a generic message that reads as a single-source
        // upgrade regardless of how many sources the filter actually matched.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, &["skills"]).expect("multi-match note");
        let action = json_confirmation_action(format!("applying pending upgrades ({note})"), true);
        assert!(
            action.contains("github.com/acme/skills"),
            "action: {action}"
        );
        assert!(
            action.contains("github.com/other/skills"),
            "action: {action}"
        );
        assert!(
            action.contains("--json is always non-interactive"),
            "action must still carry the LIFE-45 --json remedy text: {action}"
        );
    }

    #[test]
    fn json_confirmation_action_unchanged_for_single_source() {
        // spec: CLI-233 -- a single-match filter (or no filter) must not
        // widen the --json refusal's action text either.
        let action = json_confirmation_action("applying pending upgrades", true);
        assert_eq!(
            action,
            "applying pending upgrades (--json is always non-interactive; --yes is the only way to proceed)"
        );
    }
}
