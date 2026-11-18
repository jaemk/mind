//! Source pins and versions: resolving, syncing a clone to a pin, and breaking moves.

use super::*;

/// A consumer's pin intent from the `meld` CLI flags (CLI-17, CLI-200..202),
/// before it is resolved against the source. Kept distinct from [`Pin`] because a
/// freeze (`--pin`) resolves to a concrete commit only after the checkout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PinRequest {
    /// No consumer pin flag; the lower-precedence layers decide (DSC-41, LNK-3).
    #[default]
    None,
    /// Freeze to an immutable commit (CLI-200). `Some(ref)` (`--pin <ref>`)
    /// resolves and freezes that branch/tag/commit; `None` (`--pin HEAD`) freezes
    /// the floating tip the lower layers selected.
    Freeze(Option<String>),
    /// Track a moving point (CLI-201, from `--pin branch=`/`--pin tag=`): a
    /// `FollowBranch` or `Tag`, or `DefaultBranch` via a lower-precedence layer.
    Follow(Pin),
}

/// Parse the pin CLI flags into a single [`PinRequest`] (CLI-17, CLI-200..202).
/// Accepts the current `--pin` flag plus the deprecated
/// `--follow-branch`/`--pin-tag`/`--pin-ref` aliases; more than one set flag is a
/// `ConflictingPin` error. The flags are kept independent at the clap layer so
/// this structured error is what the user sees, rather than a clap usage string.
///
/// Each supplied ref value is validated with [`crate::git::validate_ref_value`] so
/// a leading-dash value is rejected with `InvalidRef` before it can reach any git
/// subprocess (DSC-66).
pub fn parse_pin_flags(
    pin: Option<String>,
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
) -> Result<PinRequest> {
    // Collect the set flags as (flag-name, request) so at-most-one is enforced
    // uniformly across `--pin` and the deprecated aliases (CLI-202).
    let mut set: Vec<(&str, PinRequest)> = Vec::new();
    if let Some(v) = pin {
        set.push(("--pin", parse_pin_value(v)?));
    }
    if let Some(b) = follow_branch {
        crate::git::validate_ref_value(&b)?;
        set.push(("--follow-branch", PinRequest::Follow(Pin::FollowBranch(b))));
    }
    if let Some(t) = pin_tag {
        crate::git::validate_ref_value(&t)?;
        set.push(("--pin-tag", PinRequest::Follow(Pin::Tag(t))));
    }
    if let Some(r) = pin_ref {
        crate::git::validate_ref_value(&r)?;
        set.push(("--pin-ref", PinRequest::Freeze(Some(r))));
    }
    if set.len() > 1 {
        return Err(MindError::ConflictingPin {
            first: set[0].0.to_string(),
            second: set[1].0.to_string(),
        });
    }
    Ok(set.into_iter().next().map_or(PinRequest::None, |(_, r)| r))
}

/// Parse the required `--pin` value into a [`PinRequest`] (CLI-200, CLI-201).
/// `HEAD` freezes the resolved tip to its commit; `branch=<name>` / `tag=<name>`
/// follow a branch / moving tag; anything else is a ref (tag/sha/branch) that is
/// resolved and frozen. A value carrying an unrecognized `key=` form is a
/// `BadPinSpec` usage error so a mistyped key (e.g. `brnach=x`) is not silently
/// treated as a ref name.
fn parse_pin_value(value: String) -> Result<PinRequest> {
    if value == "HEAD" {
        return Ok(PinRequest::Freeze(None));
    }
    if let Some(name) = value.strip_prefix("branch=") {
        crate::git::validate_ref_value(name)?;
        return Ok(PinRequest::Follow(Pin::FollowBranch(name.to_string())));
    }
    if let Some(name) = value.strip_prefix("tag=") {
        crate::git::validate_ref_value(name)?;
        return Ok(PinRequest::Follow(Pin::Tag(name.to_string())));
    }
    // spec: VER-20 VER-22 -- a range that does not parse is a usage error,
    // never a tag name.
    if let Some(range) = value.strip_prefix("version=") {
        return match crate::version::Range::parse(range) {
            Some(r) => Ok(PinRequest::Follow(Pin::Version(
                crate::source::VersionPin::unresolved(&r),
            ))),
            None => Err(MindError::BadPinSpec { value }),
        };
    }
    if value.contains('=') {
        return Err(MindError::BadPinSpec { value });
    }
    crate::git::validate_ref_value(&value)?;
    Ok(PinRequest::Freeze(Some(value)))
}

/// Resolve a consumer `--pin` request against the lower-precedence base point
/// (CLI-17, CLI-200..202): what a bare `--pin HEAD` freezes. `meld_recursive`
/// passes the curator/link/directive-resolved base (Step 2); `repin_source`
/// (CLI-209) passes the source's currently recorded pin, so a re-meld's
/// `--pin HEAD` freezes whatever the source is pinned/following today.
/// `checkout_pin` is what gets checked out; `freeze` is whether the resolved
/// commit is then persisted as an immutable `ref` pin instead of `checkout_pin`
/// itself.
pub(super) fn resolve_checkout_pin(consumer_pin: PinRequest, base_pin: Pin) -> (Pin, bool) {
    match consumer_pin {
        PinRequest::None => (base_pin, false),
        PinRequest::Follow(p) => (p, false),
        PinRequest::Freeze(Some(r)) => (Pin::Ref(r), true),
        PinRequest::Freeze(None) => (base_pin, true),
    }
}

/// A short human description of a `Pin` for the hook disclosure (HOOK-20).
/// Shared by `meld_recursive` and `upgrade` so both render the pin the same way.
pub(crate) fn pin_description(pin: &Pin) -> String {
    match pin {
        Pin::DefaultBranch => "default branch".to_string(),
        Pin::FollowBranch(b) => format!("branch {b}"),
        Pin::Tag(t) => format!("tag {t}"),
        Pin::Ref(r) => format!("ref {r}"),
        Pin::Version(v) => match &v.resolved {
            Some(resolved) => format!("version {} ({resolved})", v.range),
            None => format!("version {}", v.range),
        },
    }
}

/// `recall`'s suggestion for a source following a release tag (VER-82): when
/// `source` has a `tag` pin and the tag is a release under the
/// `release-tags` its checkout declares, the `meld --pin version=` command
/// that follows compatible releases instead. Read from the clone on disk, since
/// `recall` does not fetch.
pub(super) fn version_pin_hint(paths: &Paths, source: &crate::source::Source) -> Option<String> {
    let Pin::Tag(tag) = &source.pin else {
        return None;
    };
    let dir = clone_dir_checked(paths, source).ok()?;
    let mindfile = MindToml::load(&dir).ok()??;
    let template = mindfile
        .source
        .release_template(&dir.join("mind.toml"))
        .ok()??;
    let version = template.match_tag(tag)?;
    Some(format!(
        "hint: {} follows release tag {}; to take compatible releases, run `mind meld {} --pin version=^{version}`",
        strip_ansi(&source.name),
        strip_ansi(tag),
        crate::error::shell_quote(&strip_ansi(&source.name))
    ))
}

/// Does a recorded pin match the pin a curator or policy declares? A version
/// pin is declared by its range alone; the resolution recorded beside it is
/// not drift (VER-101, VER-110).
pub(crate) fn same_declared_pin(recorded: &Pin, declared: &Pin) -> bool {
    match (recorded, declared) {
        (Pin::Version(a), Pin::Version(b)) => a.range == b.range,
        (a, b) => a == b,
    }
}

/// Resolve a version pin's range against the remote's release tags under
/// `template` (VER-40), skipping `yanked` versions (VER-46) and warning about
/// a version whose duplicate tags disagree (VER-11).
pub(super) fn resolve_version(
    source_name: &str,
    url: &str,
    pin: &crate::source::VersionPin,
    template: &crate::version::Template,
    yanked: &[crate::version::Version],
) -> Result<crate::version::Resolution> {
    let range = pin.parsed_range().ok_or_else(|| MindError::BadPinSpec {
        value: format!("version={}", pin.range),
    })?;
    let tags = git::ls_remote_tags(url)?;
    let res = crate::version::resolve(template, &tags, &range, yanked);
    for (a, b) in &res.ambiguous {
        // spec: PRG-5 -- runs inside a counted sync step.
        crate::progress::suspend(|| {
            eprintln!(
                "warning: {}: tags {} and {} name the same version at different commits; skipping that version",
                strip_ansi(source_name),
                strip_ansi(a),
                strip_ansi(b)
            )
        });
    }
    Ok(res)
}

/// A version pin filled in from a resolution: the selected release and the
/// highest one seen (VER-30).
pub(super) fn filled_version_pin(
    pin: &crate::source::VersionPin,
    selected: &crate::version::Release,
    res: &crate::version::Resolution,
) -> crate::source::VersionPin {
    crate::source::VersionPin {
        range: pin.range.clone(),
        resolved: Some(selected.version.to_string()),
        tag: Some(selected.tag.clone()),
        highest: res.highest.as_ref().map(|r| r.version.to_string()),
        highest_tag: res.highest.as_ref().map(|r| r.tag.clone()),
        // Resolution never selects a yanked version (VER-46).
        yanked: false,
        recommended: pin.recommended.clone(),
        auto_update: pin.auto_update,
    }
}

/// A version-pinned source's release state in `--json` output (VER-91).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct VersionJson {
    pub(super) range: String,
    pub(super) resolved: Option<String>,
    pub(super) tag: Option<String>,
    pub(super) highest_available: Option<String>,
    pub(super) breaking_available: bool,
    pub(super) yanked: bool,
    /// The default branch's `pin-version` at the last resolution (VER-142).
    pub(super) recommended: Option<String>,
}

/// `pin`'s [`VersionJson`], `None` for any other pin kind. The tag is
/// source-controlled, so it is sanitized (DSC-95).
pub(super) fn version_json(pin: &Pin) -> Option<VersionJson> {
    let Pin::Version(vp) = pin else {
        return None;
    };
    Some(VersionJson {
        range: vp.range.clone(),
        resolved: vp.resolved.clone(),
        tag: vp.tag.as_deref().map(strip_ansi),
        highest_available: vp.highest.clone(),
        breaking_available: breaking_release(pin).is_some(),
        yanked: vp.yanked,
        recommended: vp.recommended.clone(),
    })
}

/// One source's [`VersionJson`] in a `sync` or `upgrade` result (VER-91).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceVersionJson {
    pub(super) source: String,
    pub(super) version: VersionJson,
}

/// The [`SourceVersionJson`] of every version-pinned source `in_scope` admits.
pub(super) fn source_versions(
    registry: &Registry,
    in_scope: &dyn Fn(&str) -> bool,
) -> Vec<SourceVersionJson> {
    registry
        .sources
        .iter()
        .filter(|s| in_scope(&s.name))
        .filter_map(|s| {
            version_json(&s.pin).map(|version| SourceVersionJson {
                source: strip_ansi(&s.name),
                version,
            })
        })
        .collect()
}

/// A breaking release `upgrade` settled (VER-61..63, VER-91): taken, or
/// listed and left.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct BreakingMoveJson {
    pub(super) source: String,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) applied: bool,
    /// The source's recommended range admits `to` (VER-141).
    pub(super) recommended: bool,
}

/// `recall`'s release lines for a version-pinned source (VER-90): a breaking
/// release outside its range, and a resolved release since yanked.
pub(super) fn version_marks(source: &crate::source::Source) -> Vec<String> {
    let Pin::Version(vp) = &source.pin else {
        return Vec::new();
    };
    let shown = strip_ansi(&source.name);
    let mut lines = Vec::new();
    if let Some((_, to)) = breaking_release(&source.pin) {
        // spec: VER-141
        let nudge = recommended_for_breaking(&source.pin)
            .map(|r| format!(" ({shown} recommends {r})"))
            .unwrap_or_default();
        lines.push(format!(
            "note: {shown}: breaking release {to} available{nudge}; take it with `mind upgrade --allow-breaking --source {}`",
            crate::error::shell_quote(&shown)
        ));
    }
    if vp.yanked
        && let Some(resolved) = &vp.resolved
    {
        lines.push(format!(
            "warning: {shown}: release {resolved} is yanked; sync moves on when a newer release in {} is tagged",
            vp.range
        ));
    }
    // spec: AUP-10 -- the declaration never changes a grant; say so once the
    // maintainer asks for more than the consumer granted.
    if let (Some(declared), Some(granted)) = (vp.auto_update, source.auto_update)
        && granted != crate::auto_update::Level::Off
        && declared > granted
    {
        lines.push(format!(
            "note: {shown} asks to update automatically, including breaking releases; allow it with `mind meld {} --auto-update=breaking`",
            crate::error::shell_quote(&shown)
        ));
    }
    lines
}

/// The release a version-pinned source is on, for `recall`'s source header
/// (VER-90): ` release 1.4.2 (^1.4.2)`. Empty for any other pin kind.
pub(super) fn version_token(pin: &Pin) -> String {
    match pin {
        Pin::Version(vp) => match &vp.resolved {
            Some(resolved) => format!(" release {resolved} ({})", vp.range),
            None => format!(" release ({})", vp.range),
        },
        _ => String::new(),
    }
}

/// The pin a source gets on `meld` when no layer names one (VER-50): when its
/// default-branch `mind.toml` declares `release-tags`, the caret of its highest
/// stable, non-yanked release that this binary meets (VER-41), already resolved
/// to that release. When none fits, the highest one. `None` when it declares no
/// `release-tags`, or declares them but no stable release matches (the source
/// then follows its default branch, and the meld says so). `dir` is the clone
/// the release tags are fetched into.
pub(super) fn default_version_pin(
    source: &crate::source::Source,
    mindfile: Option<&MindToml>,
    toml_path: &std::path::Path,
    dir: &std::path::Path,
) -> Result<Option<Pin>> {
    let Some(m) = mindfile else {
        return Ok(None);
    };
    let Some(template) = m.source.release_template(toml_path)? else {
        return Ok(None);
    };
    let yanked = m.source.yanked_versions(toml_path)?;
    let tags = git::ls_remote_tags(&source.url)?;
    let (releases, _) = crate::version::releases(&template, &tags);
    let Some(highest) = crate::version::highest_stable(&releases, &yanked) else {
        eprintln!(
            "note: {} declares release-tags but has no stable release; following the default branch",
            strip_ansi(&source.name)
        );
        return Ok(None);
    };
    // spec: VER-41 -- the default is the caret of a release this binary can
    // run, so a newest release that needs a newer mind does not strand the
    // meld on it while an older one fits.
    let stable: Vec<&crate::version::Release> = releases
        .iter()
        .filter(|r| !r.version.is_prerelease() && !yanked.contains(&r.version))
        .collect();
    let mut chosen = highest;
    if stable.len() > 1
        && let Some(fits) = stable.iter().find(|rel| {
            release_needs_newer_mind(&source.name, &source.url, dir, true, rel).is_none()
        })
    {
        chosen = *fits;
    }
    let range = crate::version::Range::caret_of(&chosen.version);
    let mut res = crate::version::resolve(&template, &tags, &range, &yanked);
    // Every release above `chosen` was skipped above.
    res.selected = Some(chosen.clone());
    for (a, b) in &res.ambiguous {
        // spec: PRG-5
        crate::progress::suspend(|| {
            eprintln!(
                "warning: {}: tags {} and {} name the same version at different commits; skipping that version",
                strip_ansi(&source.name),
                strip_ansi(a),
                strip_ansi(b)
            )
        });
    }
    let mut pin = crate::source::VersionPin::unresolved(&range);
    pin.recommended = m.source.recommended_range().map(|r| r.to_string());
    pin.auto_update = m.source.auto_update_level(toml_path)?;
    Ok(res
        .selected
        .as_ref()
        .map(|rel| Pin::Version(filled_version_pin(&pin, rel, &res))))
}

/// Honor `min-mind-version` in a resolution (VER-41): when the selected
/// release needs a newer `mind`, select the highest in-range release this
/// binary meets instead, reporting each one skipped. When none fits, the
/// highest stays selected and the DSC-40 warning applies later. `dir` holds
/// the source's git objects; `fetch` says whether it is a clone that must
/// fetch each tag first.
// spec: VER-41
pub(super) fn fit_min_mind_version(
    source_name: &str,
    url: &str,
    dir: &std::path::Path,
    fetch: bool,
    res: &mut crate::version::Resolution,
) -> Result<()> {
    // A single candidate is selected whether it fits or not.
    if res.candidates.len() < 2 {
        return Ok(());
    }
    for rel in &res.candidates {
        if release_needs_newer_mind(source_name, url, dir, fetch, rel).is_none() {
            res.selected = Some(rel.clone());
            return Ok(());
        }
    }
    res.selected = res.candidates.first().cloned();
    Ok(())
}

/// The `min-mind-version` release `rel` declares when this binary does not
/// meet it (VER-41), reporting the skip. A release whose `mind.toml` cannot be
/// read (the tag fetch fails) is warned about and treated as fitting, so a
/// flaky fetch keeps the selection rather than failing the meld or sync.
fn release_needs_newer_mind(
    source_name: &str,
    url: &str,
    dir: &std::path::Path,
    fetch: bool,
    rel: &crate::version::Release,
) -> Option<String> {
    let shown = strip_ansi(source_name);
    let text = match git::tag_mind_toml(url, dir, &rel.tag, fetch) {
        Ok(text) => text,
        Err(e) => {
            // spec: PRG-5 -- runs inside a counted sync step.
            crate::progress::suspend(|| {
                eprintln!(
                    "warning: {shown}: could not read min-mind-version of {}: {}",
                    rel.version,
                    strip_ansi(&e.to_string())
                )
            });
            return None;
        }
    };
    let required = text
        .as_deref()
        .and_then(crate::mindfile::unmet_min_version_in_text)?;
    crate::progress::suspend(|| {
        eprintln!(
            "{shown}: skipped {}: needs mind {}",
            rel.version,
            strip_ansi(&required)
        )
    });
    Some(required)
}

/// The error for a range no release satisfies (VER-4, VER-42).
pub(super) fn no_matching_release(
    source_name: &str,
    pin: &crate::source::VersionPin,
    res: &crate::version::Resolution,
) -> MindError {
    MindError::NoMatchingRelease {
        source_name: source_name.to_string(),
        range: pin.range.clone(),
        highest: res.highest.as_ref().map(|r| r.version.to_string()),
    }
}

/// The release an installed item's commit came from (VER-60): its source's
/// resolved version, when the source is version-pinned and still at that
/// commit.
pub(super) fn release_version(
    registry: &Registry,
    item: &crate::manifest::InstalledItem,
) -> Option<String> {
    let source = registry.find(&item.source)?;
    match &source.pin {
        Pin::Version(vp) if source.commit.as_deref() == Some(item.commit.as_str()) => {
            vp.resolved.clone()
        }
        _ => None,
    }
}

/// A version-pinned source's breaking release (VER-61): the highest stable
/// release seen at the last sync, when it is newer than the resolved one and
/// outside the range. An exact hold is never offered one.
fn breaking_release(pin: &Pin) -> Option<(String, crate::version::Version)> {
    let Pin::Version(vp) = pin else {
        return None;
    };
    let range = vp.parsed_range()?;
    if matches!(range, crate::version::Range::Exact(_)) {
        return None;
    }
    let highest = crate::version::Version::parse(vp.highest.as_deref()?)?;
    let resolved = vp
        .resolved
        .as_deref()
        .and_then(crate::version::Version::parse);
    if range.matches(&highest) || resolved.is_some_and(|r| r >= highest) {
        return None;
    }
    Some((vp.resolved.clone().unwrap_or_default(), highest))
}

/// The range the source recommends when it admits `pin`'s breaking release
/// (VER-141), for the "recommends ^2" hint. `None` with no breaking release,
/// no recommendation, or one that does not admit it.
pub(super) fn recommended_for_breaking(pin: &Pin) -> Option<String> {
    let Pin::Version(vp) = pin else {
        return None;
    };
    let (_, to) = breaking_release(pin)?;
    let recommended = crate::version::Range::parse(vp.recommended.as_deref()?)?;
    recommended.matches(&to).then(|| recommended.to_string())
}

/// Settle every in-scope source's breaking release before the upgrade pass
/// (VER-61..63). Each is listed; an accepted one has its range moved to the
/// caret of the new release (`^2.0.0`) and is resolved there, so its items then
/// upgrade through the ordinary pass. `--allow-breaking` accepts them all; on a
/// terminal without `--yes` or `--json` each gets its own `[y/N]` prompt;
/// otherwise they are skipped. A `preview` (CLI-263) lists them all as not
/// applied, changing nothing; the caller passes `allow` false with it.
// spec: VER-61 VER-62 VER-63
pub(super) fn settle_breaking_moves(
    paths: &Paths,
    registry: &mut Registry,
    in_scope: &dyn Fn(&str) -> bool,
    yes: bool,
    allow: bool,
    preview: bool,
    out: &crate::render::OutputCtx,
) -> Result<Vec<BreakingMoveJson>> {
    // `--json` is non-interactive (LIFE-45): a prompt would corrupt its stdout.
    let interactive = !preview && !yes && !out.json && crate::hook::is_tty();
    let mut changed = false;
    let mut moves = Vec::new();
    for source in &mut registry.sources {
        if !in_scope(&source.name) {
            continue;
        }
        let Some((from, to)) = breaking_release(&source.pin) else {
            continue;
        };
        let shown = strip_ansi(&source.name);
        // spec: VER-141 -- the source's own nudge toward the new major.
        let recommended = recommended_for_breaking(&source.pin);
        let line = match &recommended {
            Some(r) => format!("{shown} {from} -> {to} (breaking, {shown} recommends {r})"),
            None => format!("{shown} {from} -> {to} (breaking)"),
        };
        let accept = if allow {
            true
        } else if interactive {
            print_release_notes(paths, source);
            confirm(&format!("take breaking release {line}?"))?
        } else {
            false
        };
        let mut record = BreakingMoveJson {
            source: shown.clone(),
            from: from.clone(),
            to: to.to_string(),
            applied: false,
            recommended: recommended.is_some(),
        };
        if !accept {
            if !out.json {
                let nudge = recommended
                    .as_deref()
                    .map(|r| format!("; {shown} recommends {r}"))
                    .unwrap_or_default();
                println!(
                    "{} {shown} {from} -> {to} (breaking, not applied{nudge}; take it with `mind upgrade --allow-breaking --source {}`)",
                    out.warn(),
                    crate::error::shell_quote(&shown)
                );
            }
            moves.push(record);
            continue;
        }
        let Pin::Version(vp) = &source.pin else {
            continue;
        };
        let mut moved = source.clone();
        moved.pin = Pin::Version(crate::source::VersionPin {
            range: crate::version::Range::caret_of(&to).to_string(),
            resolved: None,
            tag: None,
            highest: vp.highest.clone(),
            highest_tag: vp.highest_tag.clone(),
            yanked: false,
            recommended: vp.recommended.clone(),
            auto_update: vp.auto_update,
        });
        let applied = clone_dir_checked(paths, &moved)
            .and_then(|dir| sync_clone_to_pin(&moved, &dir))
            .and_then(|(commit, pin)| match &pin {
                // Nothing selectable in the new range: the clone did not move,
                // so the old pin stays.
                Pin::Version(v) if v.resolved.is_none() => Err(MindError::NoMatchingRelease {
                    source_name: source.name.clone(),
                    range: v.range.clone(),
                    highest: v.highest.clone(),
                }),
                _ => Ok((commit, pin)),
            });
        match applied {
            Ok((commit, pin)) => {
                if !out.json {
                    println!(
                        "{} {line}: range now {}",
                        out.bullet(),
                        match &pin {
                            Pin::Version(v) => v.range.as_str(),
                            _ => "",
                        }
                    );
                }
                source.pin = pin;
                source.commit = Some(commit);
                changed = true;
                record.applied = true;
            }
            Err(e) => {
                eprintln!(
                    "  {} could not take breaking release for {shown}: {}",
                    out.err(),
                    strip_ansi(&e.to_string())
                );
            }
        }
        moves.push(record);
    }
    if changed {
        registry.save(paths)?;
    }
    Ok(moves)
}

/// The most lines of a tag message the breaking prompt shows (VER-64).
const RELEASE_NOTES_MAX_LINES: usize = 20;

/// Print the breaking release's tag message ahead of its prompt (VER-64),
/// sanitized like other source-derived text. A lightweight tag, or a message
/// that cannot be fetched, shows nothing: the notes never block the prompt.
// spec: VER-64
fn print_release_notes(paths: &Paths, source: &crate::source::Source) {
    let Pin::Version(vp) = &source.pin else {
        return;
    };
    let Some(tag) = vp.highest_tag.as_deref() else {
        return;
    };
    let notes = clone_dir_checked(paths, source)
        .and_then(|dir| git::tag_annotation(&source.url, &dir, tag));
    let Ok(Some(notes)) = notes else {
        return;
    };
    eprintln!("notes for {}:", strip_ansi(tag));
    let lines: Vec<&str> = notes.lines().collect();
    for l in lines.iter().take(RELEASE_NOTES_MAX_LINES) {
        eprintln!("  {}", strip_ansi(l));
    }
    if lines.len() > RELEASE_NOTES_MAX_LINES {
        eprintln!("  ...");
    }
}

/// Bring an existing pinned clone at `dir` to `source`'s recorded pin (CLI-55)
/// and return the new HEAD with the pin to record. A version pin is
/// re-resolved against the default branch's `release-tags` first (VER-43);
/// the range itself never changes.
pub(super) fn sync_clone_to_pin(
    source: &crate::source::Source,
    dir: &std::path::Path,
) -> Result<(String, Pin)> {
    let synced = sync_clone_to_pin_inner(source, dir)?;
    honor_full_history(source, dir)?;
    Ok(synced)
}

/// Deepen `source`'s clone at `dir` when the consumer melded it with
/// `--full-history` (DSC-121). A linked local source's "clone" is the user's
/// own working tree, which `mind` never fetches into.
// spec: DSC-121
pub(super) fn honor_full_history(
    source: &crate::source::Source,
    dir: &std::path::Path,
) -> Result<()> {
    if source.full_history && !source.is_linked() {
        git::deepen(&source.url, dir)?;
    }
    Ok(())
}

fn sync_clone_to_pin_inner(
    source: &crate::source::Source,
    dir: &std::path::Path,
) -> Result<(String, Pin)> {
    let pin = source.pin.clone();
    if !dir.join(".git").is_dir() {
        if let Some(parent) = dir.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        git::clone_at(&source.url, dir, &pin)?;
        return Ok((git::head_commit(&source.url, dir)?, pin));
    }
    let Pin::Version(vp) = &pin else {
        git::sync_to_pin(&source.url, dir, &pin)?;
        return Ok((git::head_commit(&source.url, dir)?, pin));
    };
    // spec: VER-3 -- the template and yanked list come from the default
    // branch tip, not from the release the clone is on.
    let meta = match git::default_branch_mind_toml(&source.url, dir)? {
        Some(text) => crate::mindfile::release_meta_from_text(&text, &dir.join("mind.toml"))?,
        None => crate::mindfile::ReleaseMeta::default(),
    };
    let yanked = meta.yanked;
    let template = meta.template.unwrap_or(crate::version::Template::Default);
    let mut res = resolve_version(&source.name, &source.url, vp, &template, &yanked)?;
    fit_min_mind_version(&source.name, &source.url, dir, true, &mut res)?;
    let mut next = vp.clone();
    // spec: VER-140 -- recorded fresh each sync, so a recommendation the
    // default branch dropped is dropped here too.
    next.recommended = meta.recommended;
    // spec: AUP-5 -- the declaration too, for the bare `--auto-update`, the
    // first-meld prompt, and recall's note.
    next.auto_update = meta.auto_update;
    next.highest = res.highest.as_ref().map(|r| r.version.to_string());
    next.highest_tag = res.highest.as_ref().map(|r| r.tag.clone());
    // spec: VER-90 -- recorded so `recall` marks it without a fetch.
    next.yanked = vp
        .resolved
        .as_deref()
        .and_then(crate::version::Version::parse)
        .is_some_and(|cur| yanked.contains(&cur));
    match &res.selected {
        // spec: VER-46 -- nothing in range is selectable now (every candidate
        // yanked, or the tags gone): stay where the source is.
        None => {}
        // spec: VER-46 -- a source on a since-yanked release stays there
        // until a newer in-range release appears; it never steps down.
        Some(rel)
            if vp
                .resolved
                .as_deref()
                .and_then(crate::version::Version::parse)
                .is_some_and(|cur| yanked.contains(&cur) && rel.version <= cur) => {}
        Some(rel) if vp.tag.as_deref() == Some(rel.tag.as_str()) => {
            // spec: VER-44 -- the same release; a tag that now peels to a
            // different commit is not followed.
            if let Some(recorded) = source.commit.as_deref()
                && recorded != rel.commit
            {
                // spec: PRG-5 -- runs inside a counted sync step.
                crate::progress::suspend(|| {
                    eprintln!(
                        "warning: {}: release tag {} moved from {} to {}; staying at {} (re-meld with --pin to accept it)",
                        strip_ansi(&source.name),
                        strip_ansi(&rel.tag),
                        short(recorded),
                        short(&rel.commit),
                        short(recorded)
                    )
                });
            }
        }
        Some(rel) => {
            next = filled_version_pin(&next, rel, &res);
            let pin = Pin::Version(next);
            git::sync_to_pin(&source.url, dir, &pin)?;
            return Ok((git::head_commit(&source.url, dir)?, pin));
        }
    }
    Ok((git::head_commit(&source.url, dir)?, Pin::Version(next)))
}

/// Re-pin an already-registered source (CLI-209): resolve the caller's
/// `--pin` request against the source's currently recorded pin (the base
/// point a bare `--pin HEAD` freezes, via `resolve_checkout_pin` -- the same
/// resolution `meld_recursive` performs at a first meld), re-check-out the
/// existing clone at the resolved point (or, for a source that is still
/// linked -- local and never pinned -- clone a fresh snapshot into the
/// sources tree, mirroring `meld_recursive`'s Step 3/4, so the live working
/// tree is never touched), and record the resolved pin and commit.
///
/// Transactional in the same spirit as `meld_recursive` (CLI-18): every git
/// operation runs BEFORE any field on the registered `Source` is mutated or
/// the registry is saved, so a resolution/clone failure (a bad ref, a network
/// error) leaves the source's pin, commit, and clone dir exactly as they
/// were. The POL-11/POL-20 allowlist and require-pinned gates are
/// re-evaluated here exactly as they are at a first meld, so a source whose
/// identity fell out of policy since it was melded cannot be silently
/// re-pinned around the gate.
pub(crate) fn repin_source(paths: &Paths, source_name: &str, pin: PinRequest) -> Result<()> {
    let out = crate::render::ctx();
    // spec: POL-3 -- load once; Err = invalid policy (fail closed via `?`),
    // None = unmanaged, inert.
    let policy = paths.settings().policy()?;
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        // The caller only invokes this after confirming the source is
        // registered (`is_melded`); nothing to do if it vanished underneath.
        return Ok(());
    };

    // spec: CLI-209/POL-11 -- the allowlist gate applies to a re-pin exactly
    // as it does at a first meld (meld_recursive's POL-36 check).
    if let Some(policy) = policy.as_ref() {
        let identity = source.base_identity();
        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            if let Some(path) = paths.settings().policy_path() {
                // spec: POL-37
                eprintln!("hint: managed policy at {}", path.display());
            }
            return Err(MindError::SourceNotAllowed { identity });
        }
        if !policy.lock() && !allowed {
            // POL-13: with lock off, allow is advisory; warn but proceed.
            eprintln!(
                "warning: source '{identity}' is not in the managed policy's allowlist (advisory; not enforced because [sources].lock is false)"
            );
        }
    }

    let old_pin = source.pin.clone();
    let old_commit = source.commit.clone();
    let url = source.url.clone();
    let is_local = source.is_local();
    let full_history = source.full_history;

    let (checkout_pin, freeze) = resolve_checkout_pin(pin, old_pin.clone());

    // spec: CLI-209/POL-20 -- the require-pinned gate applies identically to a
    // re-pin: a floating point (default/follow-branch) is forbidden unless the
    // request freezes it.
    if let Some(policy) = policy.as_ref()
        && !freeze
        && policy.forbids_unpinned(&checkout_pin)
    {
        return Err(MindError::UnpinnedSourceForbidden {
            identity: source_name.to_string(),
        });
    }

    // The target clone dir for a PINNED instance of this source. Stable
    // regardless of which specific pin is in effect (the sources-tree leaf
    // depends only on the identity alias, STO-59), so it can be computed
    // before the checkout runs. Force `is_linked() == false` on the probe so
    // a currently-linked local source's target resolves to the sources-tree
    // clone path, not its live working tree (mirrors meld_recursive Step 3);
    // a first-time local pin therefore clones a NEW directory rather than
    // touching the working tree the (still-linked) `url` points at.
    let mut probe = source.clone();
    if is_local {
        probe.pin = Pin::Ref(String::new());
    }
    let target = clone_dir_checked(paths, &probe)?;

    // spec: VER-80 -- a re-pin to a range resolves it the way a first meld
    // does, reading release-tags from the default branch (VER-3): the clone's
    // origin tip, or the working tree of a still-linked local source.
    let checkout_pin = match checkout_pin {
        Pin::Version(vp) => {
            let meta = if target.join(".git").is_dir() {
                match git::default_branch_mind_toml(&url, &target)? {
                    Some(text) => {
                        crate::mindfile::release_meta_from_text(&text, &target.join("mind.toml"))?
                    }
                    None => crate::mindfile::ReleaseMeta::default(),
                }
            } else {
                let root = std::path::Path::new(&url);
                let toml_path = root.join("mind.toml");
                match MindToml::load(root)? {
                    Some(m) => crate::mindfile::ReleaseMeta::from_source(&m.source, &toml_path)?,
                    None => crate::mindfile::ReleaseMeta::default(),
                }
            };
            let template = meta.template.unwrap_or(crate::version::Template::Default);
            let yanked = meta.yanked;
            let vp = crate::source::VersionPin {
                recommended: meta.recommended,
                auto_update: meta.auto_update,
                ..vp
            };
            let mut res = resolve_version(source_name, &url, &vp, &template, &yanked)?;
            if target.join(".git").is_dir() {
                fit_min_mind_version(source_name, &url, &target, true, &mut res)?;
            } else if is_local {
                fit_min_mind_version(
                    source_name,
                    &url,
                    std::path::Path::new(&url),
                    false,
                    &mut res,
                )?;
            }
            match &res.selected {
                Some(rel) => Pin::Version(filled_version_pin(&vp, rel, &res)),
                None => return Err(no_matching_release(source_name, &vp, &res)),
            }
        }
        other => other,
    };

    // Resolve/check out at the new point WITHOUT mutating the registered
    // `Source` yet (CLI-18): a failure here leaves `pin`/`commit`/the clone
    // exactly as they were.
    if target.join(".git").is_dir() {
        git::sync_to_pin(&url, &target, &checkout_pin)?;
    } else {
        // First pin on a linked local source (or a missing clone dir): clone
        // fresh; the live working tree at `url` is never touched.
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&url, &target, &checkout_pin) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(e);
        }
    }
    // spec: DSC-121 -- a re-pin may clone afresh (a first pin on a linked
    // local source), so honor --full-history here too. `target` is always a
    // sources-tree clone, never the user's working tree.
    if full_history {
        git::deepen(&url, &target)?;
    }
    let resolved_commit = git::head_commit(&url, &target)?;
    // spec: CLI-200 -- a freeze always persists the resolved commit as an
    // immutable ref, same as a first meld's freeze (Step 4).
    let final_pin = if freeze {
        Pin::Ref(resolved_commit.clone())
    } else {
        checkout_pin
    };
    let changed = old_pin != final_pin || old_commit.as_deref() != Some(resolved_commit.as_str());

    // Only now, after the checkout succeeded, persist the result.
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) else {
        return Ok(());
    };
    source.pin = final_pin.clone();
    source.commit = Some(resolved_commit);
    // spec: DSC-94 -- B4: sanitize at the sync-time re-read too.
    source.description = MindToml::load(&target)?
        .and_then(|m| m.source.description)
        .map(|d| strip_ansi(&d));
    registry.save(paths)?;

    if !out.json {
        if changed {
            println!(
                "{} re-pinned {source_name} {} -> {}",
                out.ok(),
                pin_description(&old_pin),
                pin_description(&final_pin),
            );
        } else {
            println!(
                "{} {source_name} already pinned to {}",
                out.bullet(),
                pin_description(&final_pin),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    #[test]
    fn a_release_whose_mind_toml_cannot_be_fetched_counts_as_fitting() {
        // spec: VER-41 -- a failed tag fetch warns and keeps the release
        // selectable rather than failing the resolution.
        // A repo with no `origin` remote: the fetch fails locally, never
        // reaching a network or an enclosing checkout's remote.
        let (_paths, base) = ns_paths();
        let not_a_repo = base.join("no-origin");
        std::fs::create_dir_all(&not_a_repo).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&not_a_repo)
            .status()
            .expect("run git");
        assert!(init.success());
        let rel = crate::version::Release {
            tag: "v1.0.0".into(),
            version: crate::version::Version::parse("1.0.0").unwrap(),
            commit: "0".repeat(40),
        };
        let url = not_a_repo.to_string_lossy().into_owned();
        assert_eq!(
            release_needs_newer_mind("s", &url, &not_a_repo, true, &rel),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
