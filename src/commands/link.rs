//! Item links: a deep tree/blob URL installed as a single-item source (spec/item-link.md).

use super::*;

/// The set of bare item names belonging to a source, for reference validation.
/// Every catalog item belonging to `source`, used to validate and expand an
/// item's reference tokens at install (the `{{ns:}}` names plus the `{{self}}` /
/// `{{tools:}}` / `{{path:}}` path tokens, which need each sibling's kind/bin).
pub(super) fn siblings_of(items: &[CatalogItem], source: &str) -> Vec<CatalogItem> {
    items
        .iter()
        .filter(|it| it.source == source)
        .cloned()
        .collect()
}

/// The remedy an item link's unsatisfiable reference points at (LNK-18): drop
/// the link instance, then meld the whole repo and install just this skill,
/// which brings the skill's dependency closure with it (DEP-30).
///
/// The `unmeld` step is required. The link instance is registered before the
/// install runs, so it is registered on both paths when this is printed -- the
/// warning path goes on to install the skill, the error path installs nothing
/// -- and a bare `meld ... --learn` would therefore leave a second source for
/// the same repo, and collide with the name the link already installed on the
/// warning path (NS-43/CLI-33). Removing the instance first is what makes the
/// printed command work when pasted.
///
/// `add_root` emits the `--add-root .` form (DSC-84), for the repo whose
/// declared inventory does not offer the linked skill at all -- the case item
/// links exist for. Without it the meld half of the command would fail with
/// `LearnPatternNoMatch` AFTER the unmeld half already succeeded, leaving the
/// user with the skill uninstalled, the link gone, and a stray whole-repo
/// source. The caller decides by scanning the clone
/// ([`plain_meld_reaches_link`]), never by name.
///
/// spec: DSC-95 -- the identity, the clone URL, and the item name are all
/// source-controlled, so each is sanitized and shell-quoted BEFORE being
/// composed into the command line handed to the user.
/// spec: CLI-236 -- `--learn` matches its pattern as a glob when it carries
/// glob metacharacters, and an item name may contain them (`is_safe_item_name`
/// rejects path separators and the DSC-96 blocked classes, not `*`/`?`/`[`).
/// A skill named `pdf[x]` would otherwise make this command install some OTHER
/// item, so the name is glob-escaped and the emitted pattern matches literally.
/// The pattern is also kind-qualified (`skill:`): an item link is always a
/// skill (LNK-7), and a bare name would additionally match a same-named agent
/// or rule in the repo, dropping prompt content the user never asked for.
/// The `--add-root` value that would make a plain meld reach the skill at
/// `item_path` (LNK-18, DSC-84).
///
/// An added root is convention-scanned two ways (`catalog::scan_add_roots`):
/// flat, where a skill is a bare child directory of the root, and containered,
/// where it is `<root>/skills/<name>`. Both are ONE level deep, so the root has
/// to be the skill's own parent, or its grandparent when the parent is the
/// `skills/` container. Always emitting `.` reaches only a skill sitting at
/// `<repo-root>/skills/<name>` (or flat at the repo root); for anything deeper,
/// such as `vendor/pkg/skills/foo`, the meld half of the remedy would fail with
/// `LearnPatternNoMatch` AFTER the unmeld half already succeeded, which is the
/// destroy-then-fail sequence the two-branch remedy exists to prevent.
///
/// The repo root itself is spelled `.`, which is what `validate_scan_root`
/// accepts for "scan the whole clone".
fn link_add_root(item_path: &str, kind: ItemKind) -> String {
    let item = std::path::Path::new(item_path);
    let root = match item.parent() {
        // `<...>/<kind>s/<name>` -> the parent of the container.
        Some(parent) if parent.file_name().is_some_and(|n| n == kind.dir()) => parent.parent(),
        // A flat skill dir -> its own parent.
        other => other,
    };
    let rendered = root
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if rendered.is_empty() {
        ".".to_string()
    } else {
        rendered
    }
}

/// Validate the consumer's `--kind` flag against the spec it was passed with
/// (CLI-239). The value itself is already a real item kind: clap's
/// `LinkKindArg` (agent/rule/command only) rejects anything else, with real
/// shell completions, before this ever runs.
///
/// The one usage error left to catch here, before any clone: a flag passed
/// with a spec that is not an item link (nothing else about a meld is
/// kind-scoped). Whether the kind FITS the link's shape needs the clone, so
/// it is decided later, by the catalog scan (LNK-21).
pub fn parse_link_kind(
    kind: Option<crate::cli::LinkKindArg>,
    spec: &str,
) -> Result<Option<ItemKind>> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    let parsed = kind.to_kind();
    // spec: CLI-216 -- an identity-only parse to classify the spec, not a
    // decision to clone: quiet. A spec that does not parse at all is left to
    // the meld/learn path, which reports its own error.
    if parse_spec_quiet(spec).is_ok_and(|s| s.item_path.is_none()) {
        return Err(MindError::BadKindFlag {
            value: parsed.as_str().to_string(),
            reason: format!(
                "'{}' is not an item link, and --kind applies to one only \
                 (an item link is '<repo-url>/blob/<ref>/<file>.md' or \
                 '<repo-url>/tree/<ref>/<skill-dir>')",
                strip_ansi(spec)
            ),
        });
    }
    Ok(Some(parsed))
}

/// Whether a whole-repo meld could discover the linked item at all (LNK-18).
///
/// A skill link always can: a skill is found flat (a bare child directory of a
/// scan root) as well as under a `skills/` container, so the derived
/// `--add-root` reaches it wherever it sits. A FILE link can only when its
/// parent directory is its kind's container (`agents/`, `rules/`, `commands/`):
/// convention discovery has no flat pass for file kinds, so a file anywhere
/// else is reachable only through the link.
fn link_is_conventionally_placed(source: &crate::source::Source, kind: ItemKind) -> bool {
    let Some(item_path) = source.item_path.as_deref() else {
        return true;
    };
    if !catalog::is_file_link(item_path) {
        return true;
    }
    std::path::Path::new(item_path)
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == kind.dir())
}

fn link_meld_remedy(
    identity: &str,
    url: &str,
    item_name: &str,
    kind: ItemKind,
    add_root: Option<&str>,
) -> String {
    // spec: CLI-28 -- `unmeld`'s selector is glob-aware too, and a link
    // identity embeds the skill's repo path (LNK-4), so a skill directory
    // named `pdf[x]` yields the identity `host/o/r#skills/pdf[x]`, which
    // `source_matches_glob` compiles as a PATTERN and matches against nothing:
    // the unmeld half would fail with `no melded source matches ...` and the
    // whole remedy would stop at its first command. Escaping is a no-op for an
    // identity with no metacharacters, so the ordinary form is unchanged.
    let identity = crate::error::shell_quote(&glob::Pattern::escape(&strip_ansi(identity)));
    let url = crate::error::shell_quote(&strip_ansi(url));
    let pattern = crate::error::shell_quote(&format!(
        "{}:{}",
        kind.as_str(),
        glob::Pattern::escape(&strip_ansi(item_name))
    ));
    // spec: CLI-225 -- the root is derived from the source-controlled link path,
    // so it is sanitized and shell-quoted like every other value in this command.
    let add_root = add_root
        .map(|r| format!(" --add-root {}", crate::error::shell_quote(&strip_ansi(r))))
        .unwrap_or_default();
    // `--yes` on both halves: without it a non-TTY run (a script, CI, anything
    // piping output) unmelds and then installs NOTHING while exiting 0, so the
    // printed command would silently do half its job. It skips the install
    // confirmations only; a source install hook keeps its own consent prompt
    // (HOOK-20), which `--yes` does not bypass.
    format!("mind unmeld {identity} --yes && mind meld {url}{add_root} --learn {pattern} --yes")
}

/// Whether an ordinary `meld` of the linked repo would discover the linked
/// skill by itself, deciding which [`link_meld_remedy`] form to print (LNK-18).
///
/// Decided by PATH, not by name: the clone already on disk is scanned as if it
/// were an ordinary whole-repo source (the same `Source`, with `item_path`
/// cleared), and the answer is yes only when that scan yields a `Skill` at the
/// very directory the link points at. Matching on the bare name instead would
/// call a repo reachable when its inventory declares a DIFFERENT `review`
/// (say `vendor/review`) than the one the link installed, and the remedy would
/// then quietly install the wrong skill.
///
/// A hint, not a gate: any error from the scan (an unreadable clone, a
/// malformed `mind.toml`, a version gate) answers "not reachable", so the
/// caller falls back to the `--add-root .` form, which also works on a repo
/// that would have been discovered anyway (DSC-85 drops the duplicate). It is
/// called only when a remedy is about to be printed, so an ordinary link
/// install never pays for a whole-clone scan.
fn plain_meld_reaches_link(paths: &Paths, source: &crate::source::Source, kind: ItemKind) -> bool {
    let Some(item_path) = source.item_path.as_deref() else {
        return true;
    };
    let clone_dir = source.clone_dir(paths);
    let whole = crate::source::Source {
        item_path: None,
        item_kind: None,
        curated_by: None,
        ..source.clone()
    };
    let mut items: Vec<CatalogItem> = Vec::new();
    if catalog::scan_source_at(&clone_dir, &whole, &mut items).is_err() {
        return false;
    }
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target = canon(&clone_dir.join(item_path));
    items
        .iter()
        .any(|it| it.kind == kind && canon(&it.path) == target)
}

/// Whether a `requires` entry (DEP-4) can resolve inside a single-item
/// link instance, whose only sibling is the linked skill itself (LNK-7).
///
/// A malformed or source-qualified entry returns `true` so it stays with the
/// item and install raises its specific DEP-7 cause; only an entry that is
/// well-formed and intra-source but names something other than the linked
/// skill is unsatisfiable by construction.
fn requires_resolves_alone(entry: &str, item: &CatalogItem) -> bool {
    let Ok(r) = crate::resolve::parse_item_ref(entry) else {
        return true; // InvalidRef: install reports the specific cause
    };
    if r.source.is_some() {
        return true; // CrossSource: same
    }
    r.name == item.name && r.kind.is_none_or(|k| k == item.kind)
}

/// Whether a token naming a sibling could resolve in a catalog whose only item
/// is `item` itself (LNK-7).
///
/// Mirrors the two expanders' own resolution rules: `{{ns:name}}` matches a
/// sibling of any kind by bare name (NS-11), `{{tools:name}}` matches only a
/// `Tool` (TOOL-15), and `{{path:[kind:]name}}` matches by bare name with an
/// optional kind narrowing (TOOL-18). With one item in the catalog, all three
/// reduce to "names this item, under a kind this item has".
fn sibling_token_resolves_alone(r: &crate::namespace::SiblingRef, item: &CatalogItem) -> bool {
    r.name == item.name && r.kind.is_none_or(|k| k == item.kind)
}

/// The files an item's tokens are expanded in, mirroring
/// `install::expand_references` by asking the same gate it does
/// (`namespace::item_expands_tokens`): every markdown file (NS-53), a
/// workflow's own `.js` (WF-25), and any other file the item lists in its
/// `expand:` frontmatter (NS-57). Scanning a narrower set than install expands
/// would let a reference slip past the LNK-18 check and fail later with the
/// blunt error LNK-18 exists to replace.
///
/// spec: LIFE-52 -- the tree walk is `install::collect_files`, the same capped
/// walker the install path itself uses, so this pre-install scan hits the same
/// depth cap and reports the same structured error instead of recursing
/// unbounded on a crafted source.
fn expandable_files(item: &CatalogItem) -> Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if item.path.is_dir() {
        install::collect_files(&item.path, &mut files)?;
        files.sort();
    } else {
        files.push(item.path.clone());
    }
    Ok(files
        .into_iter()
        // A single-file item (agent/rule/command/workflow) has no bundled
        // files, so only its own path applies and its markdown-ness is read
        // from that path -- which the shared gate does, given the item's root.
        // spec: NS-53 NS-57 WF-25
        .filter(|file| crate::namespace::item_expands_tokens(item, &item.path, file))
        .collect())
}

/// spec: LNK-18 -- reconcile a single-item item-link instance's intra-source
/// references before it is installed.
///
/// A link instance's catalog is exactly the linked skill (LNK-7), so a
/// reference to any other name can never resolve however the repo is laid out.
/// The two reference FORMS are treated differently, following DEP-4's own
/// distinction between them:
///
/// - A token (`{{ns:}}`, `{{tools:}}`, `{{path:}}`) is rewritten into the item's
///   text at install (NS-10/TOOL-15/TOOL-18), so it cannot be left dangling: it
///   stays a hard error, with the remedy attached (`LinkRefUnsatisfiable`)
///   instead of the blunt DEP-6/TOOL-17 `BadReference`.
/// - A `requires:` entry is pure metadata (DEP-4). It is dropped from the item,
///   recorded on the installed record, and warned about, so the skill still
///   installs rather than being unreachable by link at all.
///
/// Returns the item unchanged (borrowed) plus an empty drop list for any source
/// that is not a link instance, so the ordinary install path is untouched. The
/// drop list rides back to the caller (rather than onto the catalog item) so it
/// can be recorded on the INSTALLED record, which is what survives the command
/// (LNK-19).
pub(super) fn link_reconciled<'a>(
    paths: &Paths,
    registry: &Registry,
    item: &'a CatalogItem,
) -> Result<(std::borrow::Cow<'a, CatalogItem>, Vec<String>)> {
    let Some(source) = registry.find(&item.source) else {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    };
    if source.item_path.is_none() {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    }
    // Computed lazily: the reachability probe scans the whole clone, and most
    // link installs never print a remedy at all.
    let remedy = || {
        // spec: LNK-18 -- a file link outside its kind's container directory is
        // reachable only as a link (convention discovery finds a file item only
        // at `<root>/<kind>s/<name>.md`, and there is no flat pass for file
        // kinds), so no command is printed: one would unmeld and then fail.
        if !link_is_conventionally_placed(source, item.kind) {
            return format!(
                "this item cannot be installed any other way than as a link: the linked file \
                 sits outside a conventional {}/ directory, so no whole-repo meld discovers it. \
                 Drop the reference; or, if you can push upstream, move the file under {}/ and \
                 re-link it; or fork the repo, move the file there, and link the fork instead",
                item.kind.dir(),
                item.kind.dir()
            );
        }
        // The added root is derived from the link's own path, not fixed at `.`:
        // an added root is scanned only one level deep, so an item nested under
        // `vendor/pkg/skills/` needs that directory named (LNK-18).
        let add_root = (!plain_meld_reaches_link(paths, source, item.kind))
            .then(|| {
                source
                    .item_path
                    .as_deref()
                    .map(|p| link_add_root(p, item.kind))
            })
            .flatten();
        let command = link_meld_remedy(
            &source.name,
            &source.url,
            &item.name,
            item.kind,
            add_root.as_deref(),
        );
        format!("meld the whole repo and install just this item: `{command}`")
    };

    // A token naming anything but the linked skill is a hard stop. Scan exactly
    // the files install expands (markdown plus `expand:`-listed, NS-53/NS-57).
    for file in expandable_files(item)? {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for r in crate::namespace::sibling_reference_tokens(&text) {
            if sibling_token_resolves_alone(&r, item) {
                continue;
            }
            // spec: DSC-95 -- the token text is source-controlled and is the
            // whole `referent` field, so sanitize it here, at the boundary,
            // before it reaches the error's verbatim interpolation.
            return Err(MindError::LinkRefUnsatisfiable {
                item: item.display_key(),
                referent: strip_ansi(&r.token),
                in_source: strip_ansi(&item.source),
                remedy: remedy(),
            });
        }
    }

    // An unsatisfiable `requires` entry is metadata: record, warn, drop, install.
    let (keep, dropped): (Vec<String>, Vec<String>) = item
        .requires
        .iter()
        .cloned()
        .partition(|e| requires_resolves_alone(e, item));
    if dropped.is_empty() {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    }
    // spec: DSC-95 -- sanitize each raw entry BEFORE the join, so one entry's
    // dangling escape cannot swallow the entries after it.
    let dropped: Vec<String> = dropped.iter().map(|e| strip_ansi(e)).collect();
    let listed = dropped.join(", ");
    // spec: LNK-19 -- the warning is transient; the durable record on the
    // installed item is what `recall`/`introspect`/`--json` surface later.
    let remedy = remedy();
    crate::render::warn(format!(
        "{} declares requires {listed}, which was dropped: source {} is a single-item \
         link with no siblings, so the requirement cannot resolve. To install this item \
         together with what it requires, {remedy}",
        item.display_key(),
        strip_ansi(&item.source)
    ));
    let mut reconciled = item.clone();
    reconciled.requires = keep;
    Ok((std::borrow::Cow::Owned(reconciled), dropped))
}

/// Refuse cleanly on a platform that cannot support mind's symlink-based install
/// model (LIFE-50). On a non-unix platform the copy fallback used in place of a
/// symlink is not recognized as mind's own on a later reinstall/upgrade, which
/// surfaces as a `LinkOccupied` error; rather than that best-effort breakage, 1.0
/// refuses up front at the single item-install chokepoint. A no-op on unix.
#[cfg(unix)]
pub(super) fn require_link_platform(_paths: &Paths) -> Result<()> {
    Ok(())
}

/// See the `#[cfg(unix)]` sibling. This branch is not compiled on unix and so
/// cannot be exercised by the CI test suite; its spec ID (LIFE-50) is cited by
/// the unix-branch unit test, which pins the supported-platform half of the
/// contract.
#[cfg(not(unix))]
pub(super) fn require_link_platform(paths: &Paths) -> Result<()> {
    Err(MindError::io(
        &paths.mind_home,
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "mind requires a Unix-like platform: it links installed items with symlinks, \
             and the non-unix copy fallback is not recognized as mind's own on a later \
             reinstall or upgrade; this platform is not supported",
        ),
    ))
}

/// `mind learn <url>` (LNK-6): the one-shot form for an item link. Registers
/// the link instance when it is not yet melded, exactly as `meld <url>` would
/// (clone, hook consent, prompts), then installs its skill through the
/// standard learn path (the source-qualified `<instance>#*` selector).
#[allow(clippy::too_many_arguments)]
pub fn learn_link(
    paths: &Paths,
    url: &str,
    pin: bool,
    item_kind: Option<ItemKind>,
    home: Option<&str>,
    lobe: Option<&str>,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    let spec = parse_spec(url)?;
    if spec.item_path.is_none() {
        // A plain repo URL names a source, not an item.
        return Err(MindError::InvalidItemRef {
            name: url.to_string(),
        });
    }
    // spec: HOME-12 -- `--home` places the link instance as `meld --home`
    // does: an allowed home, checked before the clone, and moving an
    // instance already melded in another home (HOME-8).
    let home = home.map(|h| paths.allowed_home(h, "--home")).transpose()?;
    let _home_scope = home.clone().map(|h| {
        crate::paths::MeldHomeScope::set(crate::paths::MeldHome {
            home: h,
            explicit: true,
        })
    });
    // spec: LOBE-2 LOBE-7 -- a `--lobe` that is not allowed is refused before
    // the clone; recorded now so the meld below already honors it.
    // A relative one is taken against the `--home` given with it.
    let mut _lobe_scope = None;
    if let Some(l) = lobe.map(str::trim).filter(|l| !l.is_empty()) {
        let lobe_home = match &home {
            Some(h) => h.clone().unwrap_or_else(|| paths.mind_home.clone()),
            None => paths.home_of(&spec.name),
        };
        paths.resolve_lobe_in(&lobe_home, l, "--lobe")?;
        if !dry_run {
            _lobe_scope = Some(crate::paths::PendingLobeScope::set(&spec.name, l));
        }
    }
    let melded = is_melded(paths, url, None)?;
    if melded
        && !dry_run
        && let Some(requested) = &home
    {
        move_to_home(
            paths,
            &spec.name,
            requested.as_deref(),
            flow.yes,
            flow.accept,
        )?;
    }
    if !melded {
        // spec: CLI-200, LNK-3 -- `--pin` freezes the link's branch ref to its
        // current commit; without it the instance tracks the URL's ref.
        let pin_req = if pin {
            PinRequest::Freeze(None)
        } else {
            PinRequest::None
        };
        // spec: LNK-6 -- same registration as `meld <url> --register-only`;
        // the direct install below replaces the CLI-23 offer.
        meld(
            paths,
            url,
            None,
            vec![],
            vec![],
            false,
            pin_req,
            None,
            flow.accept,
            item_kind, // spec: CLI-239
        )?;
    } else {
        // spec: CLI-203 CLI-239 -- the link instance is already melded, so
        // neither `--pin` nor `--kind` has anything left to apply: both only
        // take effect at meld/registration time. Say so rather than silently
        // dropping the flag (suppressed under --json, consistent with the
        // neighboring meld notes).
        let out = crate::render::ctx();
        if !out.json {
            if pin {
                println!(
                    "note: --pin ignored; {} is already melded (pin applies only at meld time)",
                    spec.name
                );
            }
            if item_kind.is_some() {
                println!(
                    "note: --kind ignored; {} is already melded (kind applies only at meld time)",
                    spec.name
                );
            }
        }
    }
    // spec: LOBE-7 -- `--lobe` names the instance's lobe, as `meld --lobe`.
    if let Some(lobe) = lobe
        && !dry_run
    {
        set_lobe(paths, &spec.name, lobe, flow.yes, flow.accept)?;
    }
    learn(paths, &format!("{}#*", spec.name), dry_run, flow)
}

/// Whether `repo` is an item link (spec/item-link.md): a deep tree/blob URL
/// that registers a single-item source, which has no profiles (PROF-8).
pub fn is_item_link_spec(repo: &str) -> bool {
    parse_spec_quiet(repo).is_ok_and(|s| s.item_path.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    /// Which `requires` entries a single-item link instance drops, and which
    /// it keeps so install still reports their specific DEP-7 cause.
    // spec: LNK-18
    #[test]
    fn link_drops_only_the_entries_a_single_item_catalog_cannot_satisfy() {
        let item = link_skill("review", vec![]);
        // Unsatisfiable: well-formed, intra-source, names another item.
        assert!(!requires_resolves_alone("agent:dev", &item));
        assert!(!requires_resolves_alone("dev", &item));
        // The item's own name resolves (a self-require, DEP-5).
        assert!(requires_resolves_alone("review", &item));
        assert!(requires_resolves_alone("skill:review", &item));
        // Its own name under the WRONG kind is still a miss.
        assert!(!requires_resolves_alone("agent:review", &item));
        // Kept, so install raises the specific DEP-7 cause instead of a warning:
        // a source-qualified entry (CrossSource) and a malformed one (InvalidRef).
        assert!(requires_resolves_alone("owner/repo#agent:dev", &item));
        assert!(requires_resolves_alone("skill:", &item));
    }

    /// An item name carrying glob metacharacters cannot turn the remedy mind
    /// prints into a pattern that matches some OTHER item, and the pattern it
    /// prints is driven through the whole selection path, not just composed.
    // spec: LNK-18 CLI-236
    #[test]
    fn link_remedy_escapes_glob_metacharacters_in_an_item_name() {
        let remedy = link_meld_remedy(
            "local/t/repo#skills/pdf[x]",
            "/tmp/repo",
            "pdf[x]",
            ItemKind::Skill,
            None,
        );
        assert!(
            !remedy.contains("--learn 'pdf[x]'") && !remedy.contains("--learn 'skill:pdf[x]'"),
            "the raw name would be read as a glob: {remedy}"
        );
        // And it must drop the link instance first, or the meld it suggests
        // collides with the skill the link already installed.
        //
        // spec: CLI-28 -- the IDENTITY is glob-escaped for the same reason the
        // pattern is: `unmeld`'s selector is glob-aware, and a link identity
        // embeds the skill's repo path (LNK-4), so the raw
        // `local/t/repo#skills/pdf[x]` compiles as a pattern that matches no
        // source and stops the remedy at its first command.
        let escaped_id = glob::Pattern::escape("local/t/repo#skills/pdf[x]");
        assert!(
            remedy.starts_with(&format!("mind unmeld '{escaped_id}' --yes && mind meld ")),
            "the remedy must unmeld the link instance first, by an escaped selector: {remedy}"
        );
        assert!(
            crate::resolve::source_matches_glob("local/t/repo#skills/pdf[x]", &escaped_id),
            "the escaped selector must still resolve to the instance it names"
        );
        assert!(
            !crate::resolve::source_matches_glob(
                "local/t/repo#skills/pdf[x]",
                "local/t/repo#skills/pdf[x]"
            ),
            "the unescaped selector really does fail to match, which is why it is escaped"
        );
        // Both halves carry --yes, or a non-TTY paste unmelds and installs
        // nothing while still exiting 0.
        assert_eq!(
            remedy.matches("--yes").count(),
            2,
            "both halves of the remedy must be non-interactive: {remedy}"
        );
        // The escaped pattern the remedy emits still selects the literal name,
        // and only it: the raw name would have selected `pdfx` instead. And the
        // selection it produces is resolved by identity, so the metacharacters
        // are not read as a glob a second time on the way in.
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
            // An agent of the same name: a bare (kind-less) pattern would drag
            // it into the install alongside the skill (L1).
            CatalogItem {
                kind: ItemKind::Agent,
                ..catalog_skill("pdf[x]", "s", None)
            },
        ];
        let escaped = format!("skill:{}", glob::Pattern::escape("pdf[x]"));
        assert!(
            remedy.contains(&format!("--learn '{escaped}'")),
            "the remedy must carry the kind-qualified, escaped pattern: {remedy}"
        );
        let matched = learn_pattern_targets(&items, "s", &escaped).unwrap();
        assert_eq!(matched, vec![target("s", "skill:pdf[x]")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(0),
            "the remedy's own selection must resolve to the linked skill"
        );
        // Without the `skill:` qualifier the same pattern also drags in the
        // same-named agent, which is prompt content the user never asked for.
        assert_eq!(
            learn_pattern_targets(&items, "s", &glob::Pattern::escape("pdf[x]"))
                .unwrap()
                .len(),
            2,
            "the kind qualifier is what keeps the agent out"
        );
        // And the unescaped name really would have selected the other item.
        assert_eq!(
            learn_pattern_targets(&items, "s", "skill:pdf[x]").unwrap(),
            vec![target("s", "skill:pdfx")],
            "the unescaped name really would have selected the other item"
        );
    }

    /// The two remedy forms differ only by `--add-root .`, which is what makes
    /// the meld half work for a repo whose inventory does not offer the linked
    /// skill (the case item links exist for). Without it that half fails AFTER
    /// the unmeld half already ran.
    // spec: LNK-18 DSC-84
    #[test]
    fn link_remedy_adds_a_scan_root_when_a_plain_meld_would_not_reach_the_skill() {
        let plain = link_meld_remedy(
            "local/t/repo#skills/review",
            "/tmp/repo",
            "review",
            ItemKind::Skill,
            None,
        );
        let rooted = link_meld_remedy(
            "local/t/repo#skills/review",
            "/tmp/repo",
            "review",
            ItemKind::Skill,
            Some(&link_add_root("skills/review", ItemKind::Skill)),
        );
        assert_eq!(
            plain,
            "mind unmeld 'local/t/repo#skills/review' --yes && \
             mind meld '/tmp/repo' --learn 'skill:review' --yes"
        );
        assert_eq!(
            rooted,
            "mind unmeld 'local/t/repo#skills/review' --yes && \
             mind meld '/tmp/repo' --add-root '.' --learn 'skill:review' --yes"
        );
    }

    /// The `--add-root` value is derived from the linked path, because an added
    /// root is convention-scanned only one level deep: a fixed `.` reaches a
    /// skill at `<repo-root>/skills/<name>` and nothing deeper, so the meld half
    /// of the remedy would fail after the unmeld half had already run.
    // spec: LNK-18 DSC-84
    #[test]
    fn link_add_root_names_the_directory_the_scan_must_start_from() {
        // The containered layout at the repo root: the whole clone.
        assert_eq!(link_add_root("skills/review", ItemKind::Skill), ".");
        // A flat skill at the repo root: also the whole clone.
        assert_eq!(link_add_root("review", ItemKind::Skill), ".");
        // Containered, but nested: the parent of the `skills/` container, not
        // the container itself (which would make the skill a grandchild).
        assert_eq!(
            link_add_root("vendor/pkg/skills/review", ItemKind::Skill),
            "vendor/pkg"
        );
        // Flat, but nested: the skill's own parent.
        assert_eq!(link_add_root("vendor/review", ItemKind::Skill), "vendor");
        // A directory that merely ends in something skills-like is not the
        // container, so it is treated as the flat parent.
        assert_eq!(
            link_add_root("my-skills/review", ItemKind::Skill),
            "my-skills"
        );
    }

    // ----- LNK-18: the reachability probe behind the two remedy forms -----

    /// A scratch clone directory plus the linked-source record that points into
    /// it, wired so `clone_dir` resolves to the directory itself (a local
    /// source at its default pin is read live from `url`, CLI-27).
    fn link_source_at(dir: &std::path::Path, item_path: &str) -> crate::source::Source {
        crate::source::Source {
            name: format!("local/test/repo#{item_path}"),
            url: dir.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: "repo".to_string(),
            commit: None,
            description: None,
            alias: None,
            as_alias: None,
            pin: Pin::default(),
            roots: None,
            flat_skills: false,
            full_history: false,
            add_roots: None,
            item_path: Some(item_path.to_string()),
            item_kind: None,
            curated_by: None,
            origin: None,
            plugin_version: None,
            install_hooks: Vec::new(),
            items_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
            profiles: None,
            profile_snapshot: None,
            whole_snapshot: None,
            auto_update: None,
            lobe: None,
            chosen_home: false,
        }
    }

    fn write_file_at(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The whole point of deciding reachability BY PATH: a repo whose
    /// authoritative inventory declares a DIFFERENT skill under the SAME bare
    /// name as the linked one must answer "not reachable", so the remedy adds a
    /// scan root instead of quietly melding the decoy.
    // spec: LNK-18 DSC-84
    #[test]
    fn plain_meld_reaches_link_answers_by_path_not_by_bare_name() {
        let dir = scratch("lnk18-bypath");
        let paths = Paths::new(dir.join("mind"), dir.join("claude"));
        write_file_at(
            &dir.join("skills/review/SKILL.md"),
            "---\ndescription: the linked one\n---\n# review\n",
        );
        write_file_at(
            &dir.join("vendor/review/SKILL.md"),
            "---\ndescription: a decoy of the same bare name\n---\n# review\n",
        );
        // Authoritative: convention discovery is off and the only `review` the
        // whole-repo scan offers is the one at `vendor/review`.
        write_file_at(
            &dir.join("mind.toml"),
            "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"vendor/review\"\n",
        );
        let source = link_source_at(&dir, "skills/review");
        assert!(
            !plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "a same-named skill at a DIFFERENT path must not count as reachable, \
             or the remedy would install the decoy"
        );

        // Drop the authoritative inventory and the ordinary convention scan
        // finds the skill at the very path the link points at.
        std::fs::remove_file(dir.join("mind.toml")).unwrap();
        assert!(
            plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "a plain convention layout really is reachable, so the plain remedy \
             form must still be chosen for it"
        );

        // A source that is not a link instance at all short-circuits to true
        // (there is nothing to reconcile, so no remedy is ever composed).
        let ordinary = crate::source::Source {
            item_path: None,
            item_kind: None,
            curated_by: None,
            ..source.clone()
        };
        assert!(plain_meld_reaches_link(&paths, &ordinary, ItemKind::Skill));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The probe is a hint, not a gate: any error from the whole-clone scan
    /// answers "not reachable" rather than propagating, so a link install never
    /// fails because of a repo-wide scan problem it does not care about.
    // spec: LNK-18
    #[test]
    fn plain_meld_reaches_link_treats_a_failed_scan_as_not_reachable() {
        let dir = scratch("lnk18-scanfail");
        let paths = Paths::new(dir.join("mind"), dir.join("claude"));
        // The clone is gone: `scan_source_at` reports `LinkedSourceGone`
        // (CLI-212). The probe must swallow it and answer "not reachable",
        // which selects the `--add-root` form -- the form that also works on a
        // repo a plain meld would have discovered anyway (DSC-85).
        let missing = dir.join("no-such-clone");
        let source = link_source_at(&missing, "skills/review");
        assert!(
            !plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "an unreadable clone must answer 'not reachable', not propagate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- LNK-18: which files the pre-install token scan reads -----

    /// The scan reads exactly what install expands: every markdown file
    /// (NS-53) plus the non-markdown files the item lists in `expand:` (NS-57).
    /// A wider set would raise `LinkRefUnsatisfiable` for a token install never
    /// touches; a narrower one would let a real token through to the blunt
    /// error LNK-18 exists to replace.
    // spec: LNK-18
    #[test]
    fn expandable_files_reads_markdown_and_expand_listed_files_only() {
        let dir = scratch("lnk18-expandable");
        let skill = dir.join("skills/review");
        write_file_at(&skill.join("SKILL.md"), "# review\n");
        write_file_at(&skill.join("docs/notes.md"), "nested markdown\n");
        write_file_at(&skill.join("run.py"), "# {{ns:dev}}\n");
        write_file_at(&skill.join("data/blob.bin"), "not expanded\n");

        let item = CatalogItem {
            path: skill.clone(),
            expand: vec!["run.py".to_string()],
            ..link_skill("review", vec![])
        };
        let mut got: Vec<String> = expandable_files(&item)
            .unwrap()
            .iter()
            .map(|p| {
                p.strip_prefix(&skill)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec!["SKILL.md", "docs/notes.md", "run.py"],
            "markdown at any depth plus the expand:-listed file, and nothing else"
        );

        // Without the `expand:` entry the same non-markdown file drops out, so
        // the token inside it is (correctly) not the link's problem.
        let unlisted = CatalogItem {
            expand: Vec::new(),
            ..item.clone()
        };
        assert!(
            !expandable_files(&unlisted)
                .unwrap()
                .iter()
                .any(|p| p.ends_with("run.py")),
            "a non-markdown file the item does not list is not expanded, so it \
             must not be scanned either"
        );

        // A single-FILE item (an agent or rule) has no bundled tree: only its
        // own path applies, and only when it is markdown.
        let single = CatalogItem {
            kind: ItemKind::Agent,
            path: skill.join("run.py"),
            expand: vec!["run.py".to_string()],
            ..link_skill("review", vec![])
        };
        assert!(
            expandable_files(&single).unwrap().is_empty(),
            "a non-markdown single-file item has nothing install would expand"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pre-install scan walks the item tree with the SAME capped walker the
    /// install path uses, so a pathological tree gives the structured LIFE-52
    /// error instead of a silent empty scan (which would let a token slip
    /// through) or an unbounded recursion.
    // spec: LIFE-52 LNK-18
    #[test]
    fn expandable_files_depth_caps_a_pathological_item_tree() {
        let dir = scratch("lnk18-deep");
        let skill = dir.join("skills/review");
        let mut deep = skill.clone();
        for _ in 0..(install::MAX_ITEM_TREE_DEPTH + 2) {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        write_file_at(&skill.join("SKILL.md"), "# review\n");

        let item = CatalogItem {
            path: skill,
            ..link_skill("review", vec![])
        };
        let err = expandable_files(&item).expect_err("the cap must be enforced");
        assert!(
            err.to_string().contains("nested directories"),
            "the depth cap must surface as the structured LIFE-52 error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On a unix host the platform guard is a no-op, so an item install is not
    /// refused (LIFE-50). The non-unix refusal branch is `#[cfg(not(unix))]` and
    /// cannot run in CI here; this pins the supported-platform half of the
    /// contract. The ID is cited by this test, so no allowlist entry is needed.
    // spec: LIFE-50
    #[cfg(unix)]
    #[test]
    fn require_link_platform_is_ok_on_unix() {
        let paths = Paths::new(
            PathBuf::from("/tmp/mind-life50"),
            PathBuf::from("/tmp/claude-life50"),
        );
        assert!(
            require_link_platform(&paths).is_ok(),
            "the platform guard must be a no-op on unix"
        );
    }

    // ----- DSC-69: auth-failure message rendering -----
}
