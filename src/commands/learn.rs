//! `mind learn` and the item install path shared by meld, sync, and curate.

use super::*;

/// The dependency-aware plan for a `learn` selection: the rendered dependency
/// tree, whether the closure adds items beyond the explicit selection, and how
/// many items would actually be installed (the install-order length, which
/// excludes already-installed items per DEP-23).
///
/// Computed without installing, so the CLI and the interactive TUI confirm step
/// share one resolution (DEP-21): the TUI calls [`learn_preview`] for its tree.
// Consumed by the interactive TUI confirm step (DEP-40), which lands in a
// sibling change; allow until that wiring uses it.
#[allow(dead_code)]
pub struct LearnPlan {
    pub tree: String,
    pub adds_dependencies: bool,
    pub install_count: usize,
    /// The effective `kind:name` keys that would install, in dependency-first
    /// order (the same set `install_count` counts). Lets a caller union the
    /// closures of several selections instead of summing counts, which would
    /// double-count a shared dependency (CLI-236).
    pub keys: Vec<String>,
    /// The catalog items behind `keys`, in the same order: what the plan's
    /// hooks are read from (HOOK-180).
    pub items: Vec<CatalogItem>,
}

impl LearnPlan {
    /// Every hook installing this closure would offer (HOOK-180). A closure
    /// never holds an installed item (DEP-23), so none is a re-install.
    pub(super) fn hooks(
        &self,
        paths: &Paths,
        accept: crate::hook::Accept,
    ) -> Result<Vec<crate::consent::PlannedHook>> {
        let registry = Registry::load(paths)?;
        let mut plan: Vec<crate::consent::PlannedHook> = self
            .items
            .iter()
            .flat_map(|it| item_install_plan(paths, &registry, it, false, accept))
            .collect();
        // spec: HOOK-198 -- then the items hooks of the sources it installs into.
        let sources: Vec<&str> = self.items.iter().map(|it| it.source.as_str()).collect();
        plan.extend(crate::items_hook::planned(
            paths,
            &registry,
            &sources,
            crate::items_hook::PlanWhen::Touched,
            accept,
        ));
        Ok(plan)
    }
}

/// The hooks of several plans, each item once: two `--learn` matches can
/// share a dependency (CLI-236).
fn union_plan_hooks(
    paths: &Paths,
    plans: &[LearnPlan],
    accept: crate::hook::Accept,
) -> Result<Vec<crate::consent::PlannedHook>> {
    let mut seen: HashSet<crate::consent::HookKey> = HashSet::new();
    let mut hooks = Vec::new();
    for plan in plans {
        for h in plan.hooks(paths, accept)? {
            if seen.insert(h.key.clone()) {
                hooks.push(h);
            }
        }
    }
    // spec: HOOK-198 -- items hooks run after every item of the run.
    hooks.sort_by_key(|h| h.key.item.is_none() && h.key.event == "items");
    Ok(hooks)
}

/// One item of one melded source, selected by identity rather than by a ref
/// string (CLI-236).
///
/// `key` is `CatalogItem::key()`, i.e. `kind:<effective name>`. The pair is
/// carried as two fields on purpose: joining them into `<source>#<key>` and
/// re-splitting on the last `#` is lossy, because `is_safe_item_name` permits
/// `#` in an item name (it rejects path separators and the DSC-96 classes), so
/// a skill named `x#skill:review` would split as source `<src>#skill:x` and key
/// `skill:review` -- two identities that both exist and are both wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnTarget {
    pub source: String,
    pub key: String,
}

impl LearnTarget {
    /// The `<source>#<key>` spelling, for display and for a pasteable
    /// `mind learn` remedy. Never re-parsed by mind itself.
    ///
    /// spec: DSC-95 -- both halves are source-controlled, so each is sanitized
    /// BEFORE the join: running `strip_ansi` over the joined string instead
    /// would let a dangling escape in the source name swallow the `#<key>`
    /// behind it, printing a bare, unscoped `mind learn <name>`.
    ///
    /// spec: CLI-236 -- the KEY is additionally glob-escaped. mind never
    /// re-parses this string, but the user does: it is printed as a
    /// `mind learn '<source>#<key>'` they are invited to paste, and `learn`
    /// reads `*`/`?`/`[` in the name half as glob syntax (CLI-31). Unescaped,
    /// the suggested command for a matched skill named `pdf[x]` would install
    /// `pdfx` instead. Escaping is a no-op for an ordinary name. The SOURCE
    /// half is left alone: it is matched by `source_matches`, not as a glob,
    /// once the ref is split at the last `#`.
    pub(super) fn display(&self) -> String {
        format!(
            "{}#{}",
            strip_ansi(&self.source),
            glob::Pattern::escape(&strip_ansi(&self.key))
        )
    }
}

/// How a `learn` call names what it installs.
///
/// spec: CLI-236 -- a `Ref` is the user's own argument, parsed by CLI-5's
/// grammar, where `*`, `?` and `[` are glob syntax. An `Exact` selection is one
/// mind composed itself from a catalog it had just scanned, so it resolves by
/// identity: no parse, no `is_glob` test, and no round trip through a string.
/// Without that split, a `--learn` match on a skill named `pdf[x]` came back
/// through `learn` as a pattern and installed `pdfx` instead, and a match on a
/// name containing `#` resolved to a different item or to no source at all.
#[derive(Clone, Copy)]
pub(super) enum Selection<'a> {
    Ref(&'a str),
    Exact(&'a LearnTarget),
    /// Several exact targets installed as one closure, so a profile's members
    /// share one catalog scan and one manifest write (PROF-35).
    ExactMany(&'a [LearnTarget]),
}

impl Selection<'_> {
    /// The selection as shown to the user: the `--json` `target` field and the
    /// text of any message naming it. Not re-parsed anywhere.
    pub(super) fn display(&self) -> String {
        match self {
            Selection::Ref(r) => (*r).to_string(),
            Selection::Exact(t) => t.display(),
            Selection::ExactMany(ts) => ts
                .iter()
                .map(LearnTarget::display)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// The catalog index an exact selection resolves to: an identity match on
/// (source identity, effective key), with no parsing of either half.
///
/// spec: CLI-236 -- factored out of [`resolve_learn`] so the round trip a
/// `--learn` match makes (catalog -> [`LearnTarget`] -> catalog) is testable in
/// one place: it is exactly this step that used to re-parse a `<source>#<key>`
/// string and re-read a glob-metacharacter name as a pattern.
pub(super) fn exact_index(items: &[CatalogItem], target: &LearnTarget) -> Option<usize> {
    items
        .iter()
        .position(|c| c.source == target.source && c.key().as_str() == target.key)
}

/// Resolve a `learn` selection (loading the registry, scanning the catalog, and
/// running the dependency closure) without installing anything. Returns the
/// catalog, the registry, and the [`crate::deps::Resolution`] so both `learn`
/// and `learn_preview` share one computation.
fn resolve_learn(
    paths: &Paths,
    selection: Selection<'_>,
) -> Result<(Registry, Vec<CatalogItem>, crate::deps::Resolution)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;

    // Map the explicit selection to indices into `items` (by identity: a
    // CatalogItem is a unique (source, kind, name)).
    let selected_idx: Vec<usize> = match selection {
        Selection::Ref(item_ref) => {
            let parsed = parse_item_ref(item_ref)?;
            // A glob selects every match; an exact ref must resolve to one.
            let targets: Vec<&CatalogItem> = if is_glob(&parsed.name) {
                let matches = select(&items, &parsed);
                if matches.is_empty() {
                    return Err(MindError::ItemNotFound {
                        query: parsed.name.clone(),
                        sources: registry.sources.len(),
                    });
                }
                matches
            } else {
                vec![resolve(&items, &parsed, registry.sources.len())?]
            };
            targets
                .iter()
                .filter_map(|t| {
                    items
                        .iter()
                        .position(|c| c.kind == t.kind && c.name == t.name && c.source == t.source)
                })
                .collect()
        }
        // spec: CLI-236 -- an internally composed selection is matched on
        // (source identity, effective key), the same identity the manifest is
        // keyed by. A miss here is not a user typo, so it reports the key
        // rather than a pattern.
        Selection::Exact(target) => match exact_index(&items, target) {
            Some(i) => vec![i],
            None => {
                return Err(MindError::ItemNotFound {
                    query: target.display(),
                    sources: registry.sources.len(),
                });
            }
        },
        Selection::ExactMany(targets) => targets
            .iter()
            .map(|t| {
                exact_index(&items, t).ok_or_else(|| MindError::ItemNotFound {
                    query: t.display(),
                    sources: registry.sources.len(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };

    // What is already installed (manifest keys are `CatalogItem::key()` form).
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<String> = manifest.items.keys().cloned().collect();

    // `read_item_text` feeds each item's concatenated UTF-8 text to the
    // resolver so it can scan for `{{ns:}}` tokens (DEP-1): only the files
    // install expands, read in full, through the same regular-file guard.
    let resolution = crate::deps::resolve(&items, &selected_idx, &installed, read_item_text);
    Ok((registry, items, resolution))
}

/// Resolve a `learn` selection's dependency closure without installing it
/// (DEP-21). Used by the CLI dry-run/prompt path and by the interactive TUI's
/// confirm step so both compute identical trees.
// Consumed by the interactive TUI confirm step (DEP-40); allow until wired.
#[allow(dead_code)]
pub fn learn_preview(paths: &Paths, item_ref: &str) -> Result<LearnPlan> {
    learn_plan(paths, Selection::Ref(item_ref))
}

/// [`learn_preview`] for any selection form, including an exact one composed by
/// mind itself (CLI-236).
fn learn_plan(paths: &Paths, selection: Selection<'_>) -> Result<LearnPlan> {
    let (_registry, items, resolution) = resolve_learn(paths, selection)?;
    let order = resolution.install_order();
    Ok(LearnPlan {
        tree: resolution.render_tree(&items),
        adds_dependencies: resolution.adds_dependencies(),
        install_count: order.len(),
        keys: order
            .iter()
            .map(|&i| items[i].key().as_str().to_string())
            .collect(),
        items: order.iter().map(|&i| items[i].clone()).collect(),
    })
}

/// [`learn_plan`] for each of `targets`, resolved separately but against one
/// catalog scan, one manifest read, and one read of each item's text, so
/// planning a profile costs one scan however many members it has (PROF-35).
pub(super) fn learn_plans(paths: &Paths, targets: &[&LearnTarget]) -> Result<Vec<LearnPlan>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let installed: HashSet<String> = Manifest::load(paths)?.items.keys().cloned().collect();
    let texts: std::cell::RefCell<std::collections::HashMap<(String, String), String>> =
        Default::default();
    let read = |it: &CatalogItem| {
        let id = (it.source.clone(), it.key().as_str().to_string());
        texts
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| read_item_text(it))
            .clone()
    };
    targets
        .iter()
        .map(|target| {
            let idx = exact_index(&items, target).ok_or_else(|| MindError::ItemNotFound {
                query: target.display(),
                sources: registry.sources.len(),
            })?;
            let resolution = crate::deps::resolve(&items, &[idx], &installed, read);
            let order = resolution.install_order();
            Ok(LearnPlan {
                tree: resolution.render_tree(&items),
                adds_dependencies: resolution.adds_dependencies(),
                install_count: order.len(),
                keys: order
                    .iter()
                    .map(|&i| items[i].key().as_str().to_string())
                    .collect(),
                items: order.iter().map(|&i| items[i].clone()).collect(),
            })
        })
        .collect()
}

/// `mind learn <item> [--dry-run] [--yes]` — install one item, its
/// intra-source dependency closure (DEP-30), or many via a glob.
/// How to handle a link target that already exists and is not mind's own
/// (the clobber guard, LIFE-41), encountered during install.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Clobber {
    /// Refuse and surface `LinkOccupied` (the default; used by the TUI, which
    /// shows the error in the UI rather than reading a terminal prompt).
    Error,
    /// On a TTY, prompt to overwrite the conflicting target; otherwise refuse.
    Prompt,
    /// Overwrite the conflicting target without asking (`--force`).
    Force,
    /// Overwrite every conflicting target without asking, and nothing else
    /// `--force` implies (a re-meld's HOOK-60 re-offer): `--overwrite`
    /// (CLI-246).
    Overwrite,
}

impl Clobber {
    /// Whether an occupied link target is replaced without a prompt: by the
    /// flag (`--force`/`--overwrite`), or because the user answered `a` to an
    /// earlier overwrite prompt in this run (CLI-247).
    pub(super) fn overwrites(self) -> bool {
        match self {
            Clobber::Force | Clobber::Overwrite => true,
            Clobber::Prompt => OVERWRITE_ALL.load(std::sync::atomic::Ordering::Relaxed),
            Clobber::Error => false,
        }
    }
}

/// Set when the user answers `a` (all) to an overwrite prompt (CLI-247): the
/// rest of the run overwrites every conflict without asking again.
static OVERWRITE_ALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The answer to an overwrite prompt: `y` this one, `a` this and every later
/// one in the run, `n` (or Enter, or EOF) no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverwriteAnswer {
    Yes,
    All,
    No,
}

/// The replies of the `[y/N/a]` overwrite prompt (CLI-247, CLI-257).
const OVERWRITE_CHOICES: [crate::prompt::Choice<OverwriteAnswer>; 3] = [
    crate::prompt::Choice {
        keys: &["y", "yes"],
        value: OverwriteAnswer::Yes,
    },
    crate::prompt::Choice {
        keys: &["n", "no"],
        value: OverwriteAnswer::No,
    },
    crate::prompt::Choice {
        keys: &["a", "all"],
        value: OverwriteAnswer::All,
    },
];

/// Prompt `[y/N/a]` to overwrite an occupied link target (CLI-35, CLI-247).
/// An `a` sets `OVERWRITE_ALL` so later conflicts in the run do not ask.
fn confirm_overwrite(prompt: &str) -> Result<bool> {
    // spec: DSC-95 -- the prompt embeds a path; sanitize it.
    let prompt = crate::sanitize::strip_ansi(prompt);
    let answer = crate::prompt::ask(
        &format!("\n{prompt} [y/N/a] "),
        &OVERWRITE_CHOICES,
        OverwriteAnswer::No,
        "overwriting an existing link target",
    )?;
    Ok(match answer {
        OverwriteAnswer::Yes => true,
        OverwriteAnswer::All => {
            OVERWRITE_ALL.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        }
        OverwriteAnswer::No => false,
    })
}

/// The install-time options that travel together through the learn/meld chain:
/// whether to skip confirmation (`yes`), how to treat an occupied link target
/// (`clobber`), and which hook events run unattended (`accept`: the lifecycle
/// hooks, HOOK-83, and the item build hook, HOOK-74).
#[derive(Clone, Copy)]
pub struct InstallFlow {
    pub yes: bool,
    pub clobber: Clobber,
    pub accept: crate::hook::Accept,
}

/// Apply `--local` install scoping for one `learn`/`meld` invocation
/// (HARN-20/HARN-21).
///
/// When `local` is set, resolve the registered project lobe that lives AT or
/// BELOW the cwd (`Paths::detect_local_lobe`: `lobe_path.starts_with(cwd) &&
/// lobe_path != cwd`, i.e. the cwd must be an ancestor of the lobe directory,
/// not the lobe directory itself or a descendant of it) and return an RAII
/// guard that restricts the install fan-out to it (a subset of the normally-
/// resolved homes). If no registered project lobe lives under the cwd, this is
/// an error: the user asked to install locally but there is no local target.
/// An actionable note is printed to stderr first (matching the hint-then-error
/// pattern `learn` already uses for a missed item), and the returned error is
/// [`MindError::LobeTargetRequired`] -- the same "a lobe target is required"
/// condition, whose `kind` (`lobe-target-required`) is apt for `--json`
/// consumers.
///
/// When `local` is unset but the cwd IS inside a registered project lobe, print
/// a one-line note advertising `--local` (so the fan-out default stays
/// discoverable) and return `None`, leaving the global fan-out unchanged. All
/// notes are stderr-only and suppressed under `--json`.
pub fn apply_local_scope(
    paths: &Paths,
    local: bool,
) -> Result<Option<crate::paths::LocalLobeGuard>> {
    let out = crate::render::ctx();
    let detected = paths.detect_local_lobe()?;
    if local {
        match detected {
            Some(lobe) => {
                if !out.json {
                    eprintln!(
                        "note: --local -- installing into the project lobe {} only",
                        lobe.path.display()
                    );
                }
                Ok(Some(Paths::scope_to_local_lobe(lobe)))
            }
            None => {
                if !out.json {
                    // spec: CLI-232 -- M4 fix: the note and the hint now state
                    // the same rule `detect_local_lobe` actually applies
                    // (`lobe_path.starts_with(cwd) && lobe_path != cwd`): the
                    // cwd must be an ancestor of the lobe, not the lobe dir
                    // itself or somewhere below it.
                    eprintln!(
                        "note: --local installs into a project lobe that lives at or below the \
                         current directory, but no registered project lobe lives under this \
                         directory"
                    );
                    eprintln!(
                        "hint: --local matches a lobe directory strictly BELOW the current one, \
                         so run it from the project root (not from inside the lobe dir or a \
                         nested subdirectory)"
                    );
                    eprintln!(
                        "hint: register one with `mind link-project` (or `mind config lobes add`), \
                         or drop --local to install into every configured agent home"
                    );
                }
                Err(MindError::LobeTargetRequired)
            }
        }
    } else {
        if let Some(lobe) = detected
            && !out.json
        {
            eprintln!(
                "note: this directory has a registered project lobe ({}); \
                 pass --local to install only there",
                lobe.path.display()
            );
        }
        Ok(None)
    }
}

/// Record the consumer's lobe for melded source `name` (LOBE-7), from `meld
/// --lobe` / `learn <url> --lobe`: an empty `lobe` clears it. The lobe is
/// checked first (LOBE-2); then every installed item of the source whose
/// links are not where the lobe now puts them is relinked through the
/// upgrade pass (LOBE-5). Running it again changes nothing.
// spec: LOBE-7
pub(crate) fn set_lobe(
    paths: &Paths,
    name: &str,
    lobe: &str,
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let lobe = lobe.trim();
    let mut registry = Registry::load(paths)?;
    let Some(entry) = registry.sources.iter_mut().find(|s| s.name == name) else {
        return Ok(());
    };
    let wanted = if lobe.is_empty() {
        None
    } else {
        paths.resolve_lobe(name, lobe, "--lobe")?;
        Some(lobe.to_string())
    };
    crate::paths::set_consumer_lobe(name, wanted.as_deref());
    if entry.lobe != wanted {
        entry.lobe = wanted;
        registry.save(paths)?;
    }
    let keys: Vec<String> = Manifest::load(paths)?
        .items
        .values()
        .filter(|it| it.source == name)
        .map(|it| it.key().as_str().to_string())
        .collect();
    if !keys.is_empty() {
        let family = vec![name.to_string()];
        // The verb's own result is the one `--json` document (CLI-217).
        upgrade_inner_scoped(
            paths,
            yes,
            None,
            true,
            SourceScope::Names(&family, None),
            Some(&keys),
            accept,
        )?;
    }
    Ok(())
}

/// Sources whose lobe was already called out this run (LOBE-8).
static CALLED_OUT_LOBES: std::sync::Mutex<BTreeSet<String>> =
    std::sync::Mutex::new(BTreeSet::new());

/// Call out, once per source per run, a source lobe the items about to be
/// installed link into in place of the default lobe (LOBE-8). It is printed
/// with the install preview, ahead of the confirmation, in bold, and names a
/// lobe outside every mind home as such.
// spec: LOBE-8
fn call_out_lobes<'a>(paths: &Paths, items: impl IntoIterator<Item = &'a CatalogItem>) {
    let out = crate::render::ctx();
    let Ok(default) = paths.default_lobe_path() else {
        return;
    };
    for item in items {
        let Ok(Some(root)) = paths.source_lobe(&item.source) else {
            continue;
        };
        let in_effect = paths
            .agent_homes_for(&item.source)
            .is_ok_and(|ls| ls.iter().any(|l| l.path == root));
        // Keyed by source and lobe: a lobe that changes within the run (the
        // `probe` TUI) is called out again.
        let key = format!("{}\0{}", item.source, root.display());
        if !in_effect
            || !CALLED_OUT_LOBES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key)
        {
            continue;
        }
        let mut line = format!(
            "{} links its items into {} in place of {}",
            strip_ansi(&item.source),
            crate::sanitize::display_path(&root),
            crate::sanitize::display_path(&default)
        );
        if paths.lobe_outside_homes(&root) {
            line.push_str(", a directory outside every mind home");
        }
        crate::render::note(format!("{} {}", out.warn(), out.bold(&line)));
    }
}

pub fn learn(paths: &Paths, item_ref: &str, dry_run: bool, flow: InstallFlow) -> Result<()> {
    learn_selected(paths, Selection::Ref(item_ref), dry_run, flow)
}

/// [`learn`] for an exact, internally composed selection (CLI-236): the item is
/// found by identity, so nothing about its name can be re-read as a pattern.
fn learn_exact(
    paths: &Paths,
    target: &LearnTarget,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    learn_selected(paths, Selection::Exact(target), dry_run, flow)
}

pub(super) fn learn_selected(
    paths: &Paths,
    selection: Selection<'_>,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    let InstallFlow {
        yes,
        clobber,
        accept,
    } = flow;
    // spec: HOOK-181 -- the plan's answer lasts for this verb only.
    let _answers = crate::consent::scope();
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let out = crate::render::ctx();
    // The selection as text: the `--json` `target` field and the messages
    // below. Read by a human, never re-parsed by mind.
    let item_ref = selection.display();
    let item_ref = item_ref.as_str();
    let (registry, items, resolution) = resolve_learn(paths, selection).map_err(|e| {
        // spec: CLI-179 -- when sources are melded and the name is not found,
        // direct the user to `mind probe <query>` (search) rather than
        // `mind sync` (which cannot help if the item simply does not exist).
        if let MindError::ItemNotFound { ref query, sources } = e {
            // spec: CLI-225 -- `query` is the user's own `learn` argument but
            // lands verbatim in pasteable `mind probe <query>` / `mind learn
            // --all <query>` remedies below, so it is shell-quoted once here
            // before printing, matching the rule applied to source/item
            // identities elsewhere.
            let quoted_query = crate::error::shell_quote(query);
            if sources > 0 {
                eprintln!("hint: run `mind probe {quoted_query}` to search available items");
            }
            // spec: CLI-208 -- a query that names (exactly, or as an
            // unambiguous trailing suffix, CLI-5) an already-melded source is a
            // common miss: `learn <source-name>` reads like `meld`'s `<repo>`
            // argument (`install` -- the alias for `learn` -- reinforces the
            // habit), but `learn` resolves ITEM names, not source names. Point
            // at `--all`, which is what the user almost certainly wanted,
            // rather than dead-ending on "no item matches". Not printed under
            // `--json` (matches the CLI-179 hint above, which is also
            // stderr-only and not part of the JSON contract).
            if let Ok(reg) = Registry::load(paths) {
                let matching: Vec<&str> = reg
                    .sources
                    .iter()
                    .map(|s| s.name.as_str())
                    .filter(|name| source_matches(name, query))
                    .collect();
                if let [only] = matching.as_slice() {
                    eprintln!(
                        "hint: '{query}' names the melded source {only}; run \
                         `mind learn --all {quoted_query}` to install all of its items"
                    );
                }
            }
        }
        e
    })?;

    // The full closure to install, dependency-first (DEP-21, DEP-30), excluding
    // already-installed items (DEP-23).
    let order = resolution.install_order();
    let closure: Vec<&CatalogItem> = order.iter().map(|&i| &items[i]).collect();

    // CLI-157: empty closure means every requested item is already installed.
    // Treat as a distinct no-op rather than being silent or claiming "installed".
    if closure.is_empty() && !dry_run {
        if out.json {
            return print_json(&MutationResult::new("learn", item_ref, "up-to-date"));
        }
        println!("already installed; nothing to do");
        return Ok(());
    }

    // DEP-30: the collision check (CLI-33) runs over the FULL closure, not just
    // the explicit selection, so two items that would clobber each other abort
    // before anything is installed.
    if let Some((key, sources)) = colliding_install(&closure) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    // spec: DSC-99 -- name a forged namespace over the whole closure, before the
    // dry-run report as well as the install, so it reaches the user while the
    // decision to install is still theirs.
    warn_forged_namespaces(closure.iter().copied());
    // spec: LOBE-8 -- likewise a source lobe in place of the default lobe.
    call_out_lobes(paths, closure.iter().copied());

    // DEP-32: --dry-run renders the dependency tree (when deps were added) and
    // lists the full closure, installing nothing.
    if dry_run {
        if out.json {
            let mut result = MutationResult::new("learn", item_ref, "dry-run");
            // spec: DSC-95
            result.installed = closure.iter().map(|t| t.display_key()).collect();
            return print_json(&result);
        }
        if resolution.adds_dependencies() {
            print!("{}", resolution.render_tree(&items));
        }
        println!("would learn {} item(s):", closure.len());
        // spec: DSC-95 -- sanitize each cell before `print_rows` composes
        // the line.
        let rows = closure
            .iter()
            .map(|t| vec![t.display_key(), crate::sanitize::strip_ansi(&t.source)])
            .collect::<Vec<_>>();
        out.print_rows(&rows);
        return Ok(());
    }

    // DEP-31: when the closure adds items beyond the explicit selection, show the
    // tree and prompt; proceed only on a yes (or `--yes`). When it adds nothing,
    // install directly with no prompt and no tree (CLI-30 behavior unchanged).
    let asks_closure = resolution.adds_dependencies() && !yes;
    if asks_closure && (out.json || !crate::hook::is_tty()) {
        // spec: LIFE-45 CLI-259 -- `--json`, and a run without a TTY, are
        // non-interactive: refuse rather than install a whole dependency
        // closure unprompted.
        // spec: CLI-232
        return Err(MindError::ConfirmationRequired {
            action: json_confirmation_action(
                format!("installing the dependency closure of '{item_ref}'"),
                out.json,
            ),
        });
    }
    if asks_closure {
        print!("{}", resolution.render_tree(&items));
    }
    // spec: HOOK-180 HOOK-181 -- one plan for the batch: the closure question
    // (DEP-31) when there is one, and every hook the install would offer. A
    // `learn` that asks nothing today (no closure question, no hook to ask
    // about) still asks nothing.
    if !out.json {
        let mut hooks: Vec<crate::consent::PlannedHook> = closure
            .iter()
            .flat_map(|t| item_install_plan(paths, &registry, t, false, accept))
            .collect();
        // spec: HOOK-198 -- then the items hooks of the sources it installs into.
        let sources: Vec<&str> = closure.iter().map(|t| t.source.as_str()).collect();
        hooks.extend(crate::items_hook::planned(
            paths,
            &registry,
            &sources,
            crate::items_hook::PlanWhen::Touched,
            accept,
        ));
        let question = if asks_closure {
            "install this dependency closure?".to_string()
        } else {
            format!("learn these {} item(s)?", closure.len())
        };
        let listing = if asks_closure {
            Vec::new()
        } else {
            std::iter::once(format!("would learn {} item(s):", closure.len()))
                .chain(closure.iter().map(|t| {
                    format!(
                        "  {} {}",
                        t.display_key(),
                        crate::sanitize::strip_ansi(&t.source)
                    )
                }))
                .collect()
        };
        let items = (!yes).then(|| PlanItems {
            question: &question,
            always: asks_closure,
            default_yes: false,
            removal: false,
            listing,
        });
        match ask_plan(paths, items, hooks)? {
            PlanOutcome::Proceed => {}
            PlanOutcome::Cancel => {
                println!("cancelled; nothing installed");
                return Ok(());
            }
            PlanOutcome::Abort => return Ok(()),
        }
    }

    // Install each item in dependency-first order. If one fails mid-batch, stop
    // but still persist the items already installed, so the manifest always
    // matches what is on disk.
    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    let mut installed_keys: Vec<String> = Vec::new();
    for target in &closure {
        // POL-12: with the allowlist locked, skip (and report) any item whose
        // source identity is no longer allowed; install from the rest.
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&policy_identity(&registry, &target.source))
        {
            if !out.json {
                // spec: DSC-95
                println!(
                    "{} skipping {} from {}: source not permitted by the managed policy's allowlist",
                    out.warn(),
                    target.display_key(),
                    crate::sanitize::strip_ansi(&target.source)
                );
            }
            continue;
        }
        let commit = match registry.find(&target.source) {
            Some(s) => s.commit.clone().unwrap_or_default(),
            None => {
                failure = Some(MindError::SourceNotFound {
                    name: target.source.clone(),
                });
                break;
            }
        };
        // spec: LNK-18 -- a single-item link instance has no siblings, so its
        // intra-source references are reconciled (token: hard stop with the
        // remedy; requires: warn and drop) before install validates them.
        let (reconciled, dropped_requires) = match link_reconciled(paths, &registry, target) {
            Ok(c) => c,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let target: &CatalogItem = &reconciled;
        // spec: NS-41 -- refuse to install an agent whose bare harness name
        // already maps to an installed agent from a different source.
        if let Some(err) = agent_collision(paths, &manifest, target)? {
            failure = Some(err);
            break;
        }
        let siblings = siblings_of(&items, &target.source);
        let force = clobber.overwrites();
        // A `learn` never takes the re-install path: DEP-23 drops every already-
        // installed key from the closure above, so nothing reaching here is
        // installed under this effective name (HOOK-125: a repeat `learn` is a
        // no-op that runs no hook at all). The lookup stays as a defensive
        // guard, not a behavioral claim -- if the closure ever stopped
        // excluding installed items, this would still pick the update hooks.
        let is_update = manifest.items.contains_key(target.key().as_str());
        let mut result = install_item(paths, target, &commit, &siblings, force, accept, is_update);
        // CLI-35: a conflicting (non-mind) target refuses by default. With
        // `Prompt` (the default `learn`), offer to overwrite it on a TTY; on a
        // yes, retry forced, and on an `a` also stop asking (CLI-247).
        // `install` aborts before touching anything on a clobber, so the
        // retry is safe.
        if let Err(MindError::LinkOccupied { path }) = &result
            && !force
            && clobber == Clobber::Prompt
            && crate::hook::is_tty()
            && !out.json
        {
            let path = path.clone();
            // spec: DSC-95 -- sanitize the path component before
            // composing the prompt (not the whole prompt after), so the
            // trailing "exists and is not..." text cannot be swallowed by an
            // unterminated escape in the path.
            result = if confirm_overwrite(&format!(
                "{} exists and is not managed by mind; overwrite it?",
                display_path(std::path::Path::new(&path))
            ))? {
                install_item(paths, target, &commit, &siblings, true, accept, is_update)
            } else {
                Err(MindError::LinkOccupied { path })
            };
        }
        match result {
            Ok(mut installed) => {
                // spec: LNK-19 -- record what LNK-18 dropped on the installed
                // item, so the degradation outlives the install-time warning.
                installed.dropped_requires = dropped_requires;
                installed.version = release_version(&registry, &installed);
                installed_keys.push(installed.key().into());
                if !out.json {
                    // Keep the line starting with "learned <key>" (tests assert the
                    // prefix); the commit is greened (no-op when color is off).
                    println!(
                        "learned {} from {} ({})",
                        installed.display_key(),
                        installed.source,
                        out.green(&short(&installed.commit))
                    );
                }
                manifest.insert(installed);
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    manifest.save(paths)?;
    // spec: WF-24 WF-29 WF-30 WF-31 -- after the manifest is written, so the
    // warnings read the store copies and the full installed set. Reported even
    // when the batch failed part-way: the items that did install are on disk.
    warn_workflows(paths, &manifest, &installed_keys);
    // spec: LIFE-53 -- a source this install completed is now installed whole.
    let recorded = record_whole_sources(paths, Some(&items));
    let failure = failure.or(recorded.err());
    match failure {
        Some(e) => Err(e),
        None => {
            if out.json {
                let mut result = MutationResult::new("learn", item_ref, "installed");
                // DSC-95: an installed key embeds a source-controlled bare name.
                result.installed = installed_keys
                    .iter()
                    .map(|k| crate::sanitize::strip_ansi(k))
                    .collect();
                return print_json(&result);
            }
            Ok(())
        }
    }
}

/// Like `learn()` but installs silently and returns the installed keys instead
/// of emitting a JSON result. Used by `install_source_items_for_json` so the
/// meld dispatcher can fold the install outcome into ONE combined JSON object
/// (CLI-156) rather than letting `learn` emit its own separate result.
///
/// Differences from `learn()`:
/// - `dry_run` is never true (callers always want the real install).
/// - No JSON is emitted at the end; the caller receives the keys.
/// - The dep-prompt is skipped (callers always pass `yes=true` here).
pub(super) fn learn_collecting(
    paths: &Paths,
    item_ref: &str,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    learn_collecting_selected(paths, Selection::Ref(item_ref), flow)
}

/// [`learn_collecting`] for an exact, internally composed selection (CLI-236).
fn learn_collecting_exact(
    paths: &Paths,
    target: &LearnTarget,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    learn_collecting_selected(paths, Selection::Exact(target), flow)
}

pub(super) fn learn_collecting_selected(
    paths: &Paths,
    selection: Selection<'_>,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    let InstallFlow {
        clobber, accept, ..
    } = flow;
    let policy = Policy::load()?;
    let (registry, items, resolution) = resolve_learn(paths, selection)?;

    let order = resolution.install_order();
    let closure: Vec<&CatalogItem> = order.iter().map(|&i| &items[i]).collect();

    // Already all installed: nothing to collect.
    if closure.is_empty() {
        return Ok(vec![]);
    }

    if let Some((key, sources)) = colliding_install(&closure) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    let mut installed_keys: Vec<String> = Vec::new();
    for target in &closure {
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&policy_identity(&registry, &target.source))
        {
            continue; // policy-blocked items are silently skipped in collect mode
        }
        let commit = match registry.find(&target.source) {
            Some(s) => s.commit.clone().unwrap_or_default(),
            None => {
                failure = Some(MindError::SourceNotFound {
                    name: target.source.clone(),
                });
                break;
            }
        };
        // spec: LNK-18 -- same link reconciliation as `learn` (the warning it
        // may print routes to stderr under `--json`, CLI-217).
        let (reconciled, dropped_requires) = match link_reconciled(paths, &registry, target) {
            Ok(c) => c,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let target: &CatalogItem = &reconciled;
        // spec: NS-41 -- refuse to install a colliding agent (same check as learn).
        if let Some(err) = agent_collision(paths, &manifest, target)? {
            failure = Some(err);
            break;
        }
        let siblings = siblings_of(&items, &target.source);
        let force = clobber.overwrites();
        // The same defensive guard as the `learn` path above: DEP-23 has
        // already dropped every installed key, so this is false here.
        let is_update = manifest.items.contains_key(target.key().as_str());
        let result = install_item(paths, target, &commit, &siblings, force, accept, is_update);
        match result {
            Ok(mut installed) => {
                // spec: LNK-19
                installed.dropped_requires = dropped_requires;
                installed.version = release_version(&registry, &installed);
                installed_keys.push(installed.key().into());
                manifest.insert(installed);
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    manifest.save(paths)?;
    // spec: WF-31 -- the same warnings as the `learn` path above; they route to
    // stderr, which is where this flow's other advisories go under `--json`
    // (CLI-217).
    warn_workflows(paths, &manifest, &installed_keys);
    // spec: LIFE-53
    let recorded = record_whole_sources(paths, Some(&items));
    let failure = failure.or(recorded.err());
    match failure {
        Some(e) => Err(e),
        None => Ok(installed_keys),
    }
}

/// Run the post-meld auto-install flow (CLI-23) for one registered source by its
/// name: preview and prompt to install its items (`<source>#*`), or install
/// directly under `--yes`. Used for the top-level source and, via
/// `install_curated_sources`, for nested sources installed with `--recursive`
/// (DSC-55) or a curator `install = true` (DSC-58).
// spec: CLI-23
pub fn install_source_items(paths: &Paths, source_name: &str, flow: InstallFlow) -> Result<()> {
    let item_ref = format!("{source_name}#*");
    // spec: CLI-225 -- `item_ref` embeds the source-influenced `source_name`
    // and lands in pasteable `mind learn` remedies below, so it is
    // shell-quoted once here rather than framed in bare single quotes.
    let quoted_item_ref = crate::error::shell_quote(&item_ref);

    // Resolve what would install (excludes already-installed items, DEP-23). A
    // source that offers nothing matching is an ItemNotFound here; treat it as
    // "nothing to install" rather than an error.
    let plan = match learn_preview(paths, &item_ref) {
        Ok(plan) => plan,
        Err(MindError::ItemNotFound { .. }) => return Ok(()),
        Err(e) => return Err(e),
    };
    if plan.install_count == 0 {
        return Ok(());
    }

    if flow.yes {
        return learn(paths, &item_ref, false, flow);
    }
    if !crate::hook::is_tty() {
        if !json_mode() {
            println!(
                "note: registered only, nothing installed (not a TTY); {source_name} has {} item(s) to install; run `mind learn {quoted_item_ref}` (or re-meld with --yes)",
                plan.install_count
            );
        }
        return Ok(());
    }

    // Interactive: show the install preview (the dry-run list), then the plan.
    learn(paths, &item_ref, true, flow)?;
    let question = format!("install these {} item(s) now?", plan.install_count);
    // spec: HOOK-180 -- the deferred source hooks and every item's hooks, asked
    // with the CLI-23 question.
    match ask_plan(
        paths,
        Some(PlanItems {
            question: &question,
            always: true,
            default_yes: true,
            removal: false,
            listing: Vec::new(),
        }),
        plan.hooks(paths, flow.accept)?,
    )? {
        PlanOutcome::Proceed => learn(paths, &item_ref, false, InstallFlow { yes: true, ..flow }),
        PlanOutcome::Cancel => {
            println!("skipped; run `mind learn {quoted_item_ref}` to install later");
            Ok(())
        }
        PlanOutcome::Abort => Ok(()),
    }
}

/// spec: CLI-236 -- `--learn` scopes the install to the melded source's own
/// items, so the curated chain (DSC-54/55/58) is not walked and an explicit
/// `--recursive` has nothing to act on. Name it rather than dropping it
/// silently, the same disclosure `remeld` gives an inapplicable flag (CLI-206).
/// Silent under `--json`, matching the neighbouring meld notes.
pub fn note_learn_ignores_recursive(recursive: bool) {
    if recursive && !json_mode() {
        println!(
            "note: --recursive ignored; --learn installs only the matching items of the source \
             being melded, and does not walk the sources it curates. Meld again without --learn \
             to install from them, or name them directly."
        );
    }
}

/// Parse one `--learn` value into the ref it selects by and, when it carries
/// glob metacharacters, its compiled pattern (CLI-236).
///
/// Pure syntax: no catalog is consulted, so `meld` can run this BEFORE cloning
/// and reject a malformed pattern without leaving a registered source behind.
fn parse_learn_pattern(pattern: &str) -> Result<(crate::resolve::ItemRef, Option<glob::Pattern>)> {
    // spec: CLI-236
    let bad = |reason: &str| MindError::InvalidLearnPattern {
        pattern: strip_ansi(pattern),
        reason: reason.to_string(),
    };
    if pattern.trim().is_empty() {
        return Err(bad("it is empty"));
    }
    if pattern.contains('#') {
        return Err(bad(
            "it selects within the source being melded, so pass just an item name or glob \
             (e.g. 'review', 'skill:*'), not a source-qualified ref",
        ));
    }
    let parsed = crate::resolve::parse_item_ref(pattern)
        .map_err(|_| bad("it is not a valid item name, 'kind:name', or glob"))?;
    // A malformed glob must say so rather than silently degrading to an exact
    // match that then reports "matches no item", which reads as a typo in the
    // NAME rather than in the pattern syntax.
    let matcher = if crate::resolve::is_glob(&parsed.name) {
        Some(
            glob::Pattern::new(&parsed.name)
                .map_err(|e| bad(&format!("it is not a valid glob ({e})")))?,
        )
    } else {
        None
    };
    Ok((parsed, matcher))
}

/// spec: CLI-236 -- validate every `--learn` value's syntax up front, so a
/// typo'd pattern fails before `meld` clones and registers the source rather
/// than after (the clap layer cannot do this: the check is not a value parse
/// but a small grammar of its own).
pub fn validate_learn_patterns(patterns: &[String]) -> Result<()> {
    for pattern in patterns {
        parse_learn_pattern(pattern)?;
    }
    Ok(())
}

/// Resolve every `--learn` pattern against the source's own catalog and return
/// the EXACT identity of each matched item, in first-seen order, deduped
/// (CLI-236).
///
/// Matching accepts the item's BARE name as well as its effective (prefixed)
/// name. At meld time the consumer has not necessarily seen the source's
/// declared `[source].prefix` yet -- CLI-24 may prompt for it inside this very
/// command -- so requiring the effective name would make `--learn review` fail
/// on exactly the first-run case the flag exists for. Within one source the two
/// readings cannot disagree about which item is meant.
///
/// Each match is returned as a [`LearnTarget`] (source identity plus effective
/// key), not as a ref string: the install that consumes it resolves by
/// identity, so a name carrying `*`, `?`, `[` or `#` is never re-read as a
/// pattern or re-split into the wrong source.
pub(super) fn learn_pattern_targets(
    items: &[CatalogItem],
    source_name: &str,
    pattern: &str,
) -> Result<Vec<LearnTarget>> {
    // spec: CLI-236
    let (parsed, matcher) = parse_learn_pattern(pattern)?;
    let hit = |it: &CatalogItem| -> bool {
        if !parsed.kind.is_none_or(|k| it.kind == k) {
            return false;
        }
        match &matcher {
            Some(p) => p.matches(&it.effective_name()) || p.matches(&it.name),
            None => it.effective_name() == parsed.name || it.name == parsed.name,
        }
    };
    let mut targets: Vec<LearnTarget> = Vec::new();
    for it in items
        .iter()
        .filter(|it| it.source == source_name && hit(it))
    {
        // `key()` is `kind:effective_name`: exact, kind-qualified, and the same
        // identity `learn` and the manifest are keyed by.
        let t = LearnTarget {
            source: source_name.to_string(),
            key: it.key().as_str().to_string(),
        };
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    if targets.is_empty() {
        return Err(MindError::LearnPatternNoMatch {
            pattern: strip_ansi(pattern),
            source_name: strip_ansi(source_name),
        });
    }
    Ok(targets)
}

/// Resolve all `--learn` patterns for one source into a deduped target list,
/// and the subset of those targets not installed yet (CLI-236, DEP-23).
pub(super) fn learn_targets(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
) -> Result<(Vec<LearnTarget>, Vec<LearnTarget>)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let mut targets: Vec<LearnTarget> = Vec::new();
    for pattern in patterns {
        for t in learn_pattern_targets(&items, source_name, pattern)? {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
    }
    let manifest = Manifest::load(paths)?;
    let fresh: Vec<LearnTarget> = targets
        .iter()
        .filter(|t| !manifest.items.contains_key(&t.key))
        .cloned()
        .collect();
    Ok((targets, fresh))
}

/// Install only the items of a just-melded source matching the `--learn`
/// patterns (CLI-236), in place of the CLI-23 install-all offer. Each matched
/// item runs through the ordinary `learn` path, so its dependency closure
/// (DEP-30/31) and the collision checks (CLI-33) apply unchanged; the
/// preview-and-prompt gate around the batch is the CLI-23 meld gate, as for the
/// install-all offer it replaces.
///
/// Unlike that offer, a pattern matching nothing in the source is an error: the
/// user named it explicitly, so a typo must not pass as "nothing to install".
// spec: CLI-236
pub fn install_source_items_matching(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
    flow: InstallFlow,
) -> Result<()> {
    let (matched, fresh) = learn_targets(paths, source_name, patterns)?;
    if fresh.is_empty() {
        // Every match is already installed (CLI-157), reported once for the
        // batch rather than once per pattern.
        if !json_mode() {
            println!(
                "already installed; nothing to do ({} item(s))",
                matched.len()
            );
        }
        return Ok(());
    }
    // spec: CLI-225 -- each printed target embeds the source-influenced source
    // name and an item name, and lands in a pasteable `mind learn` remedy, so
    // it is sanitized (by `LearnTarget::display`, half by half per DSC-95) and
    // shell-quoted before printing.
    //
    // spec: CLI-236 -- `learn` takes exactly ONE positional (CLI-30), so a
    // space-joined list of refs is not a command: clap rejects it with
    // "unexpected argument". The fallback is a `&&`-joined SEQUENCE of
    // `mind learn` calls, which runs verbatim for one match or many, and which
    // stops at the first failure rather than continuing past it.
    let quoted = fresh
        .iter()
        .map(|t| format!("mind learn {}", crate::error::shell_quote(&t.display())))
        .collect::<Vec<_>>()
        .join(" && ");
    let install_all = |flow: InstallFlow| -> Result<()> {
        for target in &fresh {
            learn_exact(paths, target, false, flow)?;
        }
        Ok(())
    };
    if flow.yes {
        return install_all(flow);
    }
    if !crate::hook::is_tty() {
        if !json_mode() {
            println!(
                "note: registered only, nothing installed (not a TTY); {} item(s) match --learn; \
                 run `{quoted}` (or re-meld with --yes)",
                fresh.len(),
            );
        }
        return Ok(());
    }
    // Interactive: preview the whole batch (the dry-run list, dependency
    // closures included), then the plan, asked once.
    for target in &fresh {
        learn_exact(paths, target, true, flow)?;
    }
    let targets: Vec<&LearnTarget> = fresh.iter().collect();
    let hooks = union_plan_hooks(paths, &learn_plans(paths, &targets)?, flow.accept)?;
    let question = format!("install these {} item(s) now?", fresh.len());
    // spec: HOOK-180
    match ask_plan(
        paths,
        Some(PlanItems {
            question: &question,
            always: true,
            default_yes: true,
            removal: false,
            listing: Vec::new(),
        }),
        hooks,
    )? {
        PlanOutcome::Proceed => install_all(InstallFlow { yes: true, ..flow }),
        PlanOutcome::Cancel => {
            println!("skipped; run `{quoted}` to install later");
            Ok(())
        }
        PlanOutcome::Abort => Ok(()),
    }
}

/// The `--json` twin of [`install_source_items_matching`] (CLI-236): installs
/// silently and returns `(installed_keys, pending_count)` so the meld
/// dispatcher folds the outcome into ONE object (CLI-156).
///
/// `pending` counts the UNION of the matched items and their closures, not the
/// sum over patterns: without `--yes` nothing installs between patterns, so
/// summing would double-count an item two patterns both match, or a dependency
/// shared by two of them.
// spec: CLI-236 CLI-156
pub(crate) fn install_source_items_matching_for_json(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
    flow: InstallFlow,
) -> Result<(Vec<String>, usize)> {
    let (_matched, fresh) = learn_targets(paths, source_name, patterns)?;
    if fresh.is_empty() {
        return Ok((Vec::new(), 0));
    }
    if flow.yes {
        let mut installed: Vec<String> = Vec::new();
        for target in &fresh {
            installed.extend(learn_collecting_exact(paths, target, flow)?);
        }
        return Ok((installed, 0));
    }
    // No --yes in json mode: report the pending count without prompting. Union
    // each matched item's closure so a shared dependency counts once.
    let mut pending: BTreeSet<String> = BTreeSet::new();
    for target in &fresh {
        pending.extend(learn_plan(paths, Selection::Exact(target))?.keys);
    }
    Ok((Vec::new(), pending.len()))
}

/// Run the post-meld install flow (CLI-23) for a named subset of a registered
/// source's items (DSC-62). Only the items named by `bare_refs` (bare `kind:name`
/// strings in source truth) are offered; the source's other items remain
/// registered and available. The same preview-and-prompt path as
/// `install_source_items` is used, so `--yes` and `--link-only` behave
/// identically (CLI-23). An empty `bare_refs` slice installs nothing.
pub fn install_source_items_subset(
    paths: &Paths,
    source_name: &str,
    bare_refs: &[String],
    flow: InstallFlow,
) -> Result<()> {
    if bare_refs.is_empty() {
        return Ok(());
    }

    // Scan only the named source to get its items.
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(());
    };
    let source_items = catalog::scan(paths, &single(source))?;

    // Filter to only the listed bare refs.
    let subset: Vec<&CatalogItem> = select_by_bare_refs(&source_items, bare_refs);
    if subset.is_empty() {
        return Ok(());
    }

    // Build a manifest of already-installed keys to exclude them (DEP-23).
    let manifest = crate::manifest::Manifest::load(paths)?;
    let installed_keys: std::collections::HashSet<String> =
        manifest.items.keys().cloned().collect();

    // Filter to not-yet-installed items only.
    let to_install: Vec<&CatalogItem> = subset
        .into_iter()
        .filter(|it| !installed_keys.contains(it.key().as_str()))
        .collect();

    if to_install.is_empty() {
        return Ok(());
    }

    // Build a ref string that installs exactly the subset. We install each
    // item individually using the source-qualified effective name, reusing the
    // same preview-and-prompt gate that `install_source_items` uses.
    //
    // Collect effective names so the prompt message is accurate.
    let count = to_install.len();
    let refs: Vec<String> = to_install
        .iter()
        .map(|it| format!("{source_name}#{}", it.key().as_str()))
        .collect();

    if flow.yes {
        for item_ref in &refs {
            learn(paths, item_ref, false, flow)?;
        }
        return Ok(());
    }

    if !crate::hook::is_tty() {
        let ref_list = refs.join(", ");
        if !json_mode() {
            // spec: CLI-225 -- each candidate ref embeds `source_name` and an
            // item name from `it.key()`, neither restricted against shell
            // metacharacters, and lands in a pasteable `mind learn` remedy,
            // so it is shell-quoted before printing rather than framed in
            // bare single quotes.
            // spec: DSC-95 -- `shell_quote` neutralizes shell metacharacters
            // only; it does not strip an ANSI escape, which can still ride
            // out inside the single quotes it wraps `ref_str` in. Sanitize
            // first, then shell-quote the already-sanitized string.
            let ref_str = if refs.len() == 1 {
                strip_ansi(&refs[0])
            } else {
                format!("{source_name}#*")
            };
            let quoted_ref = crate::error::shell_quote(&ref_str);
            println!(
                "note: registered only, nothing installed (not a TTY); {source_name} has {count} item(s) to install; run `mind learn {quoted_ref}` (or re-meld with --yes)"
            );
            let _ = ref_list; // suppress unused warning
        }
        return Ok(());
    }

    // Interactive: preview, then the plan.
    let mut plans = Vec::new();
    for item_ref in &refs {
        learn(paths, item_ref, true, flow)?;
        plans.push(learn_preview(paths, item_ref)?);
    }
    let question = format!("install these {count} item(s) now?");
    // spec: HOOK-180
    match ask_plan(
        paths,
        Some(PlanItems {
            question: &question,
            always: true,
            default_yes: true,
            removal: false,
            listing: Vec::new(),
        }),
        union_plan_hooks(paths, &plans, flow.accept)?,
    )? {
        PlanOutcome::Proceed => {
            for item_ref in &refs {
                learn(paths, item_ref, false, InstallFlow { yes: true, ..flow })?;
            }
        }
        PlanOutcome::Cancel => {
            // spec: CLI-225 -- same rationale as above: shell-quote before
            // printing.
            let quoted_ref = crate::error::shell_quote(&format!("{source_name}#*"));
            println!("skipped; run `mind learn {quoted_ref}` to install later");
        }
        PlanOutcome::Abort => {}
    }
    Ok(())
}

// ----- Install profiles (spec/profiles.md) -----

/// Install all items from a provisioned source headlessly, for the policy
/// `auto_meld` `install = true` path (POL-58/59/60).
///
/// Each item from `source_name` that is not yet installed is attempted
/// independently via `learn` so a single failure does not abort installation of
/// the remaining items (POL-60). Build hooks are skipped unless `run_build_hooks`
/// is `true` (POL-59, same as `--accept-hooks=build`). Install
/// hooks follow the standard non-TTY path: they are skipped in a non-interactive
/// context (HOOK-72).
///
/// Returns `(installed_keys, failures)`: the effective keys installed in this
/// pass, and a `(item_key, error)` pair for each failed item so the caller can
/// apply POL-34/POL-60 soft-fail semantics (warn, record, continue, non-zero
/// exit). Empty failures means all items installed successfully (or there were
/// no items to install).
///
/// spec: CLI-217 -- under `--json` the per-item install runs through the SILENT
/// `learn_collecting`, and the keys ride back to `sync` to be folded into its
/// one result object. Calling `learn` there emitted one `learn` document per
/// provisioned item ahead of sync's own, which is N+1 documents on the stdout
/// of exactly the unattended fleet run `auto_meld` exists for.
// spec: POL-58 POL-59 POL-60
pub fn install_provisioned_items(
    paths: &Paths,
    source_name: &str,
    run_build_hooks: bool,
) -> (Vec<String>, Vec<(String, MindError)>) {
    let flow = InstallFlow {
        yes: true,
        clobber: Clobber::Prompt,
        accept: crate::hook::Accept {
            build: run_build_hooks,
            ..crate::hook::Accept::default()
        },
    };

    // Load the registry and scan items for this source only.
    let registry = match Registry::load(paths) {
        Ok(r) => r,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    let Some(source) = registry.find(source_name) else {
        return (vec![], vec![]);
    };
    // spec: CLI-213 POL-60 -- a single-source scan, so it must not degrade past a
    // `LinkedSourceGone` source: `catalog::scan` would return `Ok(vec![])` and this
    // function would report "nothing to install" for a source that could not be
    // read at all, so POL-60's warn-record-continue-nonzero accounting would record
    // nothing. A scan failure is a recorded `(source, err)` failure instead.
    let source_items = match scan_one(paths, source) {
        Ok(items) => items,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    if source_items.is_empty() {
        return (vec![], vec![]);
    }

    // Load the manifest once to know what is already installed; `learn` will
    // also check per call, but this avoids the overhead of starting `learn`
    // for items that are clearly already done.
    let manifest = match Manifest::load(paths) {
        Ok(m) => m,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    let installed: std::collections::HashSet<String> = manifest.items.keys().cloned().collect();

    // Install each not-yet-installed item independently so a failure on one
    // does not block the rest (POL-60).
    let json = json_mode();
    let mut failures = Vec::new();
    let mut installed_keys: Vec<String> = Vec::new();
    for item in &source_items {
        let key = item.key();
        if installed.contains(key.as_str()) {
            continue; // already installed; no-op
        }
        let item_ref = format!("{source_name}#{}", key.as_str());
        // spec: CLI-217 -- silent under `--json` (keys are returned instead of
        // a per-item document); the ordinary `learn` in text mode, so an
        // interactive `mind sync` still narrates what it installed.
        let outcome = if json {
            learn_collecting(paths, &item_ref, flow).map(|keys| installed_keys.extend(keys))
        } else {
            learn(paths, &item_ref, false, flow)
        };
        if let Err(e) = outcome {
            // spec: DSC-95 -- `key` is source-controlled; sanitize before it
            // rides into the (source_name, key) pair that the `sync` caller
            // below prints raw otherwise.
            failures.push((key.display(), e));
        }
    }
    (installed_keys, failures)
}

/// Install `source_name`'s items silently (no JSON emitted by `learn`) and
/// return `(installed_keys, pending_count)`. Used in `--json` mode so the
/// meld dispatcher can emit ONE combined JSON object (CLI-156).
///
/// - `flow.yes = true`: installs everything, returns `(keys, 0)`.
/// - `flow.yes = false`: returns `([], N)` where N is the pending item count
///   without prompting (json mode is always non-interactive).
pub(crate) fn install_source_items_for_json(
    paths: &Paths,
    source_name: &str,
    flow: InstallFlow,
) -> Result<(Vec<String>, usize)> {
    let item_ref = format!("{source_name}#*");
    let plan = match learn_preview(paths, &item_ref) {
        Ok(plan) => plan,
        Err(MindError::ItemNotFound { .. }) => return Ok((vec![], 0)),
        Err(e) => return Err(e),
    };
    if plan.install_count == 0 {
        return Ok((vec![], 0));
    }
    if flow.yes {
        let keys = learn_collecting(paths, &item_ref, flow)?;
        return Ok((keys, 0));
    }
    // No --yes in json mode: report the pending count without prompting.
    Ok((vec![], plan.install_count))
}

/// If two selected items would install under the same `kind:name`, return that
/// key and the sources that collide on it.
pub(super) fn colliding_install(targets: &[&CatalogItem]) -> Option<(String, Vec<String>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in targets {
        // Group by the raw (identity) key: sanitizing before grouping could
        // merge two distinct identities that differ only by a stripped
        // character, silently changing which items are reported as colliding.
        by_key
            .entry(t.key().as_str().to_string())
            .or_default()
            .push(t.source.clone());
    }
    by_key
        .into_iter()
        .find(|(_, sources)| sources.len() > 1)
        .map(|(key, sources)| {
            // spec: DSC-95 -- sanitize for display here, at the boundary:
            // both callers feed this straight into `MindError::AmbiguousItem`,
            // whose `#[error(...)]` Display joins `candidates` (and interpolates
            // `query`) verbatim, with no sanitizing step of its own.
            (
                crate::sanitize::strip_ansi(&key),
                sources
                    .iter()
                    .map(|s| crate::sanitize::strip_ansi(s))
                    .collect(),
            )
        })
}

/// Install one item, then run its install hook (HOOK-81) as the final step. On a
/// hook failure, roll the just-installed item back (remove its links and store
/// copy via the file registry) so it is left not installed, then propagate the
/// error. `accept` names the item hook events (HOOK-83) and the build hook
/// (HOOK-74) that run unattended.
///
/// `is_update` says this install replaces an existing install of the same
/// effective name, in which case the item's update hooks run in place of its
/// install hooks when it declares any (HOOK-125). In practice that is
/// `upgrade`'s in-place swap: the `learn` paths drop already-installed items
/// from the closure before reaching here (CLI-157, DEP-23), and an upgrade
/// RENAME is a removal plus a first install, not a re-install.
#[allow(clippy::too_many_arguments)]
pub(super) fn install_item(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &[CatalogItem],
    force: bool,
    accept: crate::hook::Accept,
    is_update: bool,
) -> Result<crate::manifest::InstalledItem> {
    // spec: LIFE-50 -- refuse cleanly on a platform mind's symlink model does not
    // support, before creating (or reusing) any link.
    require_link_platform(paths)?;
    // spec: LOBE-8 -- every install path (the `--json` and profile installs
    // do not pass through the `learn` preview) names the lobe before the
    // first link; once per source and lobe, so a preview's callout stands.
    call_out_lobes(paths, std::iter::once(item));
    // spec: HOOK-125 -- on a re-install the item's update hooks replace them.
    let install_hooks = item.install_time_hooks(is_update);
    let hook_env = if item.build.is_some() || !install_hooks.is_empty() {
        catalog_item_hook_env(paths, item)?
    } else {
        crate::hook::HookEnv::default()
    };
    let mut installed = install::install(
        paths,
        item,
        commit,
        siblings,
        force,
        accept.build,
        &hook_env,
    )?;
    // HOOK-86: run every resolved install hook in declaration order (the scalar
    // shorthand is folded in as the first required hook). On a hook failure, roll
    // the just-installed item back.
    if !install_hooks.is_empty() {
        let store = paths.mind_home.join(&installed.store);
        match install::run_item_install_hooks(
            item,
            &install_hooks,
            &store,
            commit,
            accept,
            &hook_env,
        ) {
            // spec: HOOK-110 -- persist the ran/skipped outcome on the item's
            // manifest record so a later `hooks run` (HOOK-102) does not treat
            // an already-run (or already-offered) hook as never having been
            // offered.
            Ok(recorded) => installed.install_hooks = recorded,
            Err(e) => {
                let _ = install::uninstall(paths, &installed);
                return Err(e);
            }
        }
    }
    Ok(installed)
}

/// Run an item's uninstall hooks (HOOK-82, HOOK-86) in declaration order if any
/// are declared, then remove the item via its file registry. `uninstall_hooks`
/// come from the live source catalog (nothing is recorded for them, HOOK-84) and
/// are empty when the source or item is no longer available, in which case removal
/// proceeds with no hook. A hook failure propagates BEFORE removal, leaving the
/// item installed.
pub(super) fn uninstall_item(
    paths: &Paths,
    item: &crate::manifest::InstalledItem,
    uninstall_hooks: &[&crate::mindfile::ResolvedHook],
    commit: &str,
    accept: crate::hook::Accept,
) -> Result<()> {
    if !uninstall_hooks.is_empty() {
        let store = paths.mind_home.join(&item.store);
        if store.exists() {
            let hook_env = item_hook_env(paths, &item.source, item.profiles.clone())?;
            install::run_item_uninstall_hooks(
                item,
                uninstall_hooks,
                &store,
                commit,
                accept,
                &hook_env,
            )?;
        }
    }
    install::uninstall(paths, item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    /// The `<source>#<key>` spelling mind prints for the user to paste survives
    /// the round trip back through `learn`'s own ref grammar.
    // spec: CLI-236 DSC-95
    #[test]
    fn learn_target_display_escapes_a_key_the_user_would_paste_back() {
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
        ];
        let target = target("s", "skill:pdf[x]");
        let printed = target.display();
        // mind never re-parses this, but the user pastes it into `mind learn`,
        // which DOES read the name half as a glob. Unescaped it would select
        // the wrong item, exactly as the `--learn` pattern would.
        let parsed = crate::resolve::parse_item_ref(&printed).expect("a valid ref");
        let selected = crate::resolve::select(&items, &parsed);
        assert_eq!(
            selected
                .iter()
                .map(|it| it.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pdf[x]"],
            "the printed ref must select the item it names: {printed}"
        );
        // And the unescaped spelling really would have selected the other one.
        let raw = format!("{}#{}", target.source, target.key);
        let raw_parsed = crate::resolve::parse_item_ref(&raw).expect("a valid ref");
        assert_eq!(
            crate::resolve::select(&items, &raw_parsed)
                .iter()
                .map(|it| it.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pdfx"],
            "the unescaped spelling selects the wrong item, which is why it is escaped"
        );
    }

    /// A `--learn` pattern matches the bare name as well as the effective one,
    /// is scoped to the melded source, and yields exact kind-qualified
    /// identities.
    // spec: CLI-236
    #[test]
    fn learn_pattern_matches_bare_and_effective_names_within_the_source() {
        let items = vec![
            catalog_skill("review", "github.com/o/r", Some("jk")),
            catalog_skill("extra", "github.com/o/r", Some("jk")),
            catalog_skill("review", "github.com/other/r", None),
        ];
        // The bare name resolves even though the item installs as `jk:review`,
        // which is what a first-time melder can actually type.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "review").unwrap(),
            vec![target("github.com/o/r", "skill:jk:review")]
        );
        // So does the effective name.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "jk:review").unwrap(),
            vec![target("github.com/o/r", "skill:jk:review")]
        );
        // A glob matches either reading, and never reaches another source.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "*e*").unwrap(),
            vec![
                target("github.com/o/r", "skill:jk:review"),
                target("github.com/o/r", "skill:jk:extra"),
            ]
        );
        // A kind qualifier that does not match filters everything out.
        assert!(matches!(
            learn_pattern_targets(&items, "github.com/o/r", "agent:review"),
            Err(MindError::LearnPatternNoMatch { .. })
        ));
    }

    /// A matched selection resolves back to the SAME catalog item, whatever the
    /// item is named. The selection is carried as an identity, so the two
    /// characters that used to break the round trip -- a glob metacharacter
    /// (re-read as a pattern) and a `#` (re-split into the wrong source) --
    /// resolve exactly.
    // spec: CLI-236
    #[test]
    fn a_matched_selection_round_trips_to_the_same_item() {
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
            catalog_skill("x#skill:review", "s", None),
            catalog_skill("review", "s", None),
        ];
        // A glob-metacharacter name, selected by the escaped literal pattern
        // the LNK-18 remedy emits, resolves back to itself and not to its
        // near-twin.
        let escaped = format!("skill:{}", glob::Pattern::escape("pdf[x]"));
        let matched = learn_pattern_targets(&items, "s", &escaped).unwrap();
        assert_eq!(matched, vec![target("s", "skill:pdf[x]")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(0),
            "the selection must resolve back to `pdf[x]`, not `pdfx`"
        );
        // A `#`-carrying name is reachable only by a glob (a `#` in the pattern
        // itself is refused as a source-qualified ref), and its selection must
        // still resolve to that item.
        let matched = learn_pattern_targets(&items, "s", "skill:x*").unwrap();
        assert_eq!(matched, vec![target("s", "skill:x#skill:review")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(2),
            "the `#`-carrying selection must resolve to its own item"
        );
        // The failure the identity carry exists to prevent: that same key,
        // joined into `<source>#<key>` and split on the last `#` the way the
        // ref round trip did, names a DIFFERENT item that also exists.
        let joined = matched[0].display();
        let (_src, key) = joined.rsplit_once('#').unwrap();
        assert_eq!(
            key, "skill:review",
            "the joined spelling really is ambiguous: {joined}"
        );
        assert_ne!(
            exact_index(&items, &matched[0]),
            exact_index(&items, &target("s", "skill:review")),
            "the two readings must be different items"
        );
    }

    /// The pattern forms `--learn` refuses, each with its own reason.
    // spec: CLI-236
    #[test]
    fn learn_pattern_rejects_unusable_values() {
        let items = vec![catalog_skill("review", "github.com/o/r", None)];
        let reason = |p: &str| match learn_pattern_targets(&items, "github.com/o/r", p) {
            Err(MindError::InvalidLearnPattern { reason, .. }) => reason,
            other => panic!("expected InvalidLearnPattern for {p:?}, got {other:?}"),
        };
        assert!(reason("").contains("empty"));
        assert!(reason("   ").contains("empty"));
        assert!(reason("other/repo#review").contains("selects within the source being melded"));
        assert!(reason("[bad").contains("not a valid glob"));
    }

    /// The miss half of the exact-selection lookup. Unreachable in practice --
    /// the target is composed from the very catalog it is looked up in, one
    /// statement earlier -- but it is what stops a future caller that composes
    /// a target from a stale scan from silently installing the wrong item.
    // spec: CLI-236
    #[test]
    fn exact_index_misses_a_target_that_is_not_in_the_catalog() {
        let items = vec![
            catalog_skill("review", "s", None),
            catalog_skill("review", "other", None),
        ];
        assert_eq!(exact_index(&items, &target("s", "skill:review")), Some(0));
        // Right key, wrong source: identity is the PAIR, so this is a miss and
        // not a fallback onto the same-named item of another source.
        assert_eq!(exact_index(&items, &target("nope", "skill:review")), None);
        // Right source, wrong kind: `key()` is kind-qualified.
        assert_eq!(exact_index(&items, &target("s", "agent:review")), None);
        // Right source, unknown name.
        assert_eq!(exact_index(&items, &target("s", "skill:absent")), None);
    }

    /// Answer the overwrite prompt's choices with `input`, as the terminal would.
    fn overwrite_answer(input: &str) -> OverwriteAnswer {
        let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        crate::prompt::ask_from(
            &mut reader,
            &mut out,
            "overwrite? [y/N/a] ",
            &OVERWRITE_CHOICES,
            OverwriteAnswer::No,
        )
        .unwrap()
    }

    #[test]
    fn overwrite_prompt_answers_yes_all_or_no() {
        // spec: CLI-247 CLI-257 CLI-258 - `a` accepts this conflict and every
        // later one; Enter and EOF take the default (refuse); an unrecognized
        // reply is asked again rather than refusing.
        assert_eq!(overwrite_answer("y\n"), OverwriteAnswer::Yes);
        assert_eq!(overwrite_answer(" YES \n"), OverwriteAnswer::Yes);
        assert_eq!(overwrite_answer("a\n"), OverwriteAnswer::All);
        assert_eq!(overwrite_answer("All\n"), OverwriteAnswer::All);
        assert_eq!(overwrite_answer("\n"), OverwriteAnswer::No);
        assert_eq!(overwrite_answer(""), OverwriteAnswer::No);
        assert_eq!(overwrite_answer("n\n"), OverwriteAnswer::No);
        assert_eq!(overwrite_answer("maybe\ny\n"), OverwriteAnswer::Yes);
        // The flags overwrite without a prompt; `Error` never does.
        assert!(Clobber::Force.overwrites());
        assert!(Clobber::Overwrite.overwrites());
        assert!(!Clobber::Error.overwrites());
    }
}
