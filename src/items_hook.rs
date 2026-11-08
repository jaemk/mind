//! Items hooks (spec/install-hooks.md, HOOK-190..197): a source-level hook that
//! runs once per command with the source's whole installed set, so its author
//! can keep host state that depends on the set (a generated registry, a shared
//! resource) in step with it.
//!
//! The hook's run state is the set it last applied (HOOK-195), recorded on the
//! source's registry entry. A hook is pending while the source's installed set
//! differs from that record, and every offer hands the hook the difference
//! against it, so a skipped or failed run is caught up by the next one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::manifest::Manifest;
use crate::mindfile::{HookEvent, MindToml};
use crate::paths::Paths;
use crate::sanitize::strip_ansi;
use crate::source::{HookOrigin, RecordedItemsHook, Registry, Source};

/// The event word, in disclosures, notes, and the hook results.
const EVENT: &str = "items";

/// One installed item as the items document lists it (HOOK-193).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEntry {
    pub kind: String,
    /// The bare name.
    pub name: String,
    pub effective_name: String,
    /// The absolute store path. For a removed item, the path it had.
    pub store: String,
    /// The item's recorded reasons (PROF-20).
    pub profiles: Vec<String>,
}

impl ItemEntry {
    /// The identity a set is compared by: kind and effective name.
    fn key(&self) -> (String, String) {
        (self.kind.clone(), self.effective_name.clone())
    }
}

/// One item of a recorded applied set: the entry plus its content hash, which
/// is what makes a content-only change show up as `updated` (HOOK-191).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedItem {
    #[serde(flatten)]
    pub entry: ItemEntry,
    pub hash: String,
}

/// The items document an items hook reads from `MIND_ITEMS_FILE` (HOOK-193).
#[derive(Debug, Serialize)]
struct ItemsDoc<'a> {
    version: u8,
    source: &'a str,
    commit: &'a str,
    command: &'a str,
    added: Vec<&'a ItemEntry>,
    updated: Vec<&'a ItemEntry>,
    removed: Vec<&'a ItemEntry>,
    installed: Vec<&'a ItemEntry>,
}

/// The difference between a hook's applied set and the current one.
#[derive(Debug, Default, PartialEq, Eq)]
struct Delta<'a> {
    added: Vec<&'a ItemEntry>,
    updated: Vec<&'a ItemEntry>,
    removed: Vec<&'a ItemEntry>,
}

impl Delta<'_> {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }

    fn summary(&self) -> String {
        format!(
            "{} added, {} updated, {} removed",
            self.added.len(),
            self.updated.len(),
            self.removed.len()
        )
    }
}

/// The difference from `applied` (`None`: never applied, read as empty) to
/// `current` (HOOK-193). Each list keeps `current`'s (or, for `removed`,
/// `applied`'s) order, which is sorted by kind then effective name.
fn delta<'a>(applied: Option<&'a [AppliedItem]>, current: &'a [AppliedItem]) -> Delta<'a> {
    let applied = applied.unwrap_or_default();
    let before: BTreeMap<(String, String), &AppliedItem> =
        applied.iter().map(|a| (a.entry.key(), a)).collect();
    let after: BTreeMap<(String, String), &AppliedItem> =
        current.iter().map(|a| (a.entry.key(), a)).collect();
    let mut d = Delta::default();
    for item in current {
        match before.get(&item.entry.key()) {
            None => d.added.push(&item.entry),
            Some(prev) if prev.hash != item.hash => d.updated.push(&item.entry),
            Some(_) => {}
        }
    }
    for item in applied {
        if !after.contains_key(&item.entry.key()) {
            d.removed.push(&item.entry);
        }
    }
    d
}

/// The installed set of `source` as recorded in `manifest`, sorted by kind
/// then effective name (HOOK-193).
fn installed_set(paths: &Paths, manifest: &Manifest, source: &str) -> Vec<AppliedItem> {
    let mut set: Vec<AppliedItem> = manifest
        .items
        .values()
        .filter(|it| it.source == source)
        .map(|it| AppliedItem {
            entry: ItemEntry {
                kind: it.kind.as_str().to_string(),
                name: it.bare_name.clone(),
                effective_name: it.name.clone(),
                store: paths.mind_home.join(&it.store).display().to_string(),
                profiles: it.profiles.clone(),
            },
            hash: it.hash.clone(),
        })
        .collect();
    set.sort_by_key(|a| a.entry.key());
    set
}

/// Every source's installed items, keyed `kind:name` with their hashes: what a
/// command's before and after are compared by to find the sources it touched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<String, BTreeMap<String, String>>);

/// Snapshot the manifest. Best effort: an unreadable manifest reads as empty,
/// and the verb that runs next reports it.
pub fn snapshot(paths: &Paths) -> Snapshot {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    if let Ok(manifest) = Manifest::load(paths) {
        for it in manifest.items.values() {
            out.entry(it.source.clone())
                .or_default()
                .insert(it.key().into(), it.hash.clone());
        }
    }
    Snapshot(out)
}

impl Snapshot {
    fn source(&self, name: &str) -> Option<&BTreeMap<String, String>> {
        self.0.get(name)
    }
}

/// An items hook in effect for a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemsHook {
    pub run: String,
    pub label: String,
    pub optional: bool,
}

/// The items hooks in effect for `source` (HOOK-190): the ones its clone's
/// `mind.toml` declares, else, for a source with no `mind.toml` (the DSC-60
/// gate), the curated ones recorded at `meld`. A manifest that does not parse
/// declares nothing, failing closed as the other source hooks do.
pub fn effective_hooks(paths: &Paths, source: &Source) -> Vec<ItemsHook> {
    let dir = source.clone_dir(paths);
    match MindToml::load(&dir) {
        Ok(Some(mf)) => mf
            .resolved_hooks(&dir.join("mind.toml"))
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.event == HookEvent::Items)
            .map(|h| ItemsHook {
                label: h.label().to_string(),
                run: h.run,
                optional: h.optional,
            })
            .collect(),
        Ok(None) => source
            .items_hooks
            .iter()
            .filter(|r| r.origin == Some(HookOrigin::Curated))
            .map(|r| ItemsHook {
                run: r.command.clone(),
                label: r
                    .name
                    .as_deref()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(&r.command)
                    .to_string(),
                optional: r.optional,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Record a curator's items hooks on `source` at `meld` (HOOK-190, HOOK-127),
/// keeping the applied set of a hook that is already recorded.
pub fn record_curated(source: &mut Source, hooks: &[crate::mindfile::ResolvedHook]) {
    for h in hooks.iter().filter(|h| h.event == HookEvent::Items) {
        let rec = record_mut(source, &h.run);
        rec.name = h.name.clone();
        rec.optional = h.optional;
        rec.origin = Some(HookOrigin::Curated);
    }
}

/// The record for `command`, created empty when absent.
fn record_mut<'a>(source: &'a mut Source, command: &str) -> &'a mut RecordedItemsHook {
    let idx = match source.items_hooks.iter().position(|r| r.command == command) {
        Some(i) => i,
        None => {
            source.items_hooks.push(RecordedItemsHook {
                command: command.to_string(),
                applied: None,
                name: None,
                optional: false,
                origin: None,
            });
            source.items_hooks.len() - 1
        }
    };
    &mut source.items_hooks[idx]
}

/// A hook's status line for `hooks list` (HOOK-196).
pub fn status(paths: &Paths, manifest: &Manifest, source: &Source, command: &str) -> String {
    let current = installed_set(paths, manifest, &source.name);
    let applied = source
        .items_hooks
        .iter()
        .find(|r| r.command == command)
        .and_then(|r| r.applied.as_deref());
    let d = delta(applied, &current);
    let recorded = applied.map(<[AppliedItem]>::len).unwrap_or(0);
    if d.is_empty() {
        format!("applied ({recorded} items)")
    } else {
        format!("pending ({}; {recorded} items applied)", d.summary())
    }
}

/// The tally an offer reports, for `hooks run`'s HOOK-107 accounting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub existed: usize,
    pub ran: usize,
    pub skipped_for_consent: usize,
    /// Hooks held back because nothing was pending.
    pub settled: usize,
}

/// Offer `source_name`'s items hooks (HOOK-191..195): each pending one, or
/// every one under `force`, is disclosed, decided, and run in the source's
/// clone with the items document. `command` is the verb the document names.
///
/// A successful run records the set it was given; a skip, decline, or abort
/// leaves the record, so the hook stays pending. A failure is returned after
/// the registry is saved, without touching the command's item changes.
pub fn offer(
    paths: &Paths,
    source_name: &str,
    command: &str,
    accept: crate::hook::Accept,
    force: bool,
) -> Result<Tally> {
    let mut tally = Tally::default();
    let mut registry = Registry::load(paths)?;
    let Some(idx) = registry.sources.iter().position(|s| s.name == source_name) else {
        return Ok(tally);
    };
    let hooks = effective_hooks(paths, &registry.sources[idx]);
    if hooks.is_empty() {
        return Ok(tally);
    }
    let manifest = Manifest::load(paths)?;
    let current = installed_set(paths, &manifest, source_name);
    let mut dirty = false;

    for hook in &hooks {
        let source = &registry.sources[idx];
        let applied = source
            .items_hooks
            .iter()
            .find(|r| r.command == hook.run)
            .and_then(|r| r.applied.as_deref());
        let d = delta(applied, &current);
        if d.is_empty() && !force {
            tally.settled += 1;
            continue;
        }
        tally.existed += 1;

        let dir = source.clone_dir(paths);
        let commit = source.commit.clone().unwrap_or_default();
        let clone_path = dir.display().to_string();
        let browse_url = source.browse_url(&commit);
        let disclosure = crate::hook::hook_disclosure_text(
            &hook.label,
            EVENT,
            hook.optional,
            &source.name,
            &crate::commands::pin_description(&source.pin),
            &commit,
            &clone_path,
            &hook.run,
            None,
            browse_url.as_deref(),
        );
        let site = crate::hook_log::Site {
            item: None,
            commit: source.commit.as_deref(),
            replaced: &[],
        };
        let key = crate::consent::HookKey::source_hook(&source.name, EVENT, &hook.run);
        // spec: TUI-44 HOOK-198 -- a captured probe action cannot prompt here:
        // the refusal would come after the run's items changed, and the rerun
        // on the terminal changes nothing to offer it for. A hook no plan
        // answered stays pending instead.
        let planned = matches!(
            crate::consent::answer_for(&key),
            Some(crate::consent::Answer::RunAll | crate::consent::Answer::SkipAll)
        );
        let unasked = crate::prompt::captured() && !accept.items && !planned;
        let act = if unasked {
            crate::hook::HookAct::Skip
        } else {
            crate::hook::decide(&key, &disclosure, hook.optional, accept.items)?
        };
        if act != crate::hook::HookAct::Run {
            if act == crate::hook::HookAct::Abort {
                crate::hook_log::aborted(&source.name, site, EVENT, &hook.label, &hook.run);
            } else if unasked {
                // Nobody was asked, so it is a skip, not a decline (HOOK-160).
                crate::hook_log::file(crate::hook_log::base(
                    &source.name,
                    site,
                    EVENT,
                    &hook.label,
                    &hook.run,
                    crate::hook_log::Outcome::Skipped,
                ));
            } else {
                crate::hook_log::not_run(&source.name, site, EVENT, &hook.label, &hook.run);
            }
            if crate::hook::is_tty() && !unasked {
                crate::render::note(format!(
                    "note: skipped items hook '{}' for {}; it stays pending",
                    strip_ansi(&hook.label),
                    strip_ansi(&source.name)
                ));
            } else {
                tally.skipped_for_consent += 1;
                let why = if unasked {
                    "probe cannot prompt"
                } else {
                    "not a terminal"
                };
                crate::render::note(skip_note(&hook.label, &source.name, why));
            }
            // The record exists from the first offer (so `recall --sources`
            // counts the hook), but its applied set is left as it was.
            let run = hook.run.clone();
            record_mut(&mut registry.sources[idx], &run);
            dirty = true;
            continue;
        }

        tally.ran += 1;
        // A progress announcement, like the other hooks' `running ... hook`
        // lines; under `--json` fd 1 points at stderr (CLI-217).
        println!(
            "running items hook '{}' for {} ({})",
            strip_ansi(&hook.label),
            strip_ansi(&source.name),
            d.summary()
        );
        let doc = ItemsDoc {
            version: 1,
            source: &source.name,
            commit: &commit,
            command,
            added: d.added.clone(),
            updated: d.updated.clone(),
            removed: d.removed.clone(),
            installed: current.iter().map(|a| &a.entry).collect(),
        };
        let result = write_doc(paths, &doc).and_then(|file| {
            let mut env = crate::commands::source_hook_env(source);
            env.items_file = Some(file.clone());
            let result = crate::hook::run_hook(
                &hook.run,
                &dir,
                &source.name,
                EVENT,
                &hook.label,
                &env,
                site,
            );
            let _ = std::fs::remove_file(&file);
            result
        });
        // Either error keeps the records of the hooks that already ran.
        if let Err(e) = result {
            if dirty {
                registry.save(paths)?;
            }
            return Err(e);
        }
        record_mut(&mut registry.sources[idx], &hook.run).applied = Some(current.clone());
        dirty = true;
    }
    if dirty {
        registry.save(paths)?;
    }
    Ok(tally)
}

/// Write the items document to a fresh file under mind's temp dir.
fn write_doc(paths: &Paths, doc: &ItemsDoc) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let dir = paths.tmp_dir().join("items-hooks");
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::MindError::io(&dir, e))?;
    let file = dir.join(format!(
        "{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let body = serde_json::to_vec_pretty(doc).expect("the items document serializes");
    std::fs::write(&file, body).map_err(|e| crate::error::MindError::io(&file, e))?;
    Ok(file)
}

/// The note for an items hook skipped for want of a terminal, with the
/// remedy (HOOK-106's shape): the identity is shell-quoted in the command.
fn skip_note(label: &str, source_name: &str, why: &str) -> String {
    let label = strip_ansi(label);
    let source_name = strip_ansi(source_name);
    let quoted = crate::error::shell_quote(&source_name);
    format!(
        "note: skipped items hook '{label}' for {source_name} ({why}); it stays \
         pending. Run it unattended with:\n  mind hooks run {quoted} --event items \
         --accept-hooks=items"
    )
}

/// Which of a source's items hooks a plan lists (HOOK-198).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanWhen {
    /// Every one: the run changes the source's installed set.
    Touched,
    /// Only those pending now: an `upgrade` catching up an earlier skip.
    Pending,
    /// Those an emptied set leaves pending: an `unmeld` removing every item.
    Emptied,
}

/// The plan entries for `sources`' items hooks (HOOK-198), each source once,
/// in the order given. An items hook runs in the source's clone after the
/// run's items, so there is nothing separate to inspect.
// spec: HOOK-198
pub fn planned<S: AsRef<str>>(
    paths: &Paths,
    registry: &Registry,
    sources: impl IntoIterator<Item = S>,
    when: PlanWhen,
    accept: crate::hook::Accept,
) -> Vec<crate::consent::PlannedHook> {
    let manifest = match when {
        PlanWhen::Pending => Manifest::load(paths).ok(),
        PlanWhen::Touched | PlanWhen::Emptied => None,
    };
    let policy = crate::policy::Policy::load().ok().flatten();
    let mut seen = std::collections::BTreeSet::new();
    let mut plan = Vec::new();
    for name in sources {
        let name = name.as_ref();
        if !seen.insert(name.to_string()) {
            continue;
        }
        let Some(source) = registry.find(name) else {
            continue;
        };
        // The settle skips it (POL-12), so the plan does not list it.
        if !policy_permits(policy.as_ref(), source) {
            continue;
        }
        let current = match (when, &manifest) {
            (PlanWhen::Pending, Some(m)) => Some(installed_set(paths, m, name)),
            (PlanWhen::Pending, None) => continue,
            (PlanWhen::Emptied, _) => Some(Vec::new()),
            (PlanWhen::Touched, _) => None,
        };
        let commit = source.commit.clone().unwrap_or_default();
        let dir = source.clone_dir(paths);
        for hook in effective_hooks(paths, source) {
            if let Some(current) = &current {
                let applied = source
                    .items_hooks
                    .iter()
                    .find(|r| r.command == hook.run)
                    .and_then(|r| r.applied.as_deref());
                if delta(applied, current).is_empty() {
                    continue;
                }
            }
            plan.push(crate::consent::PlannedHook {
                key: crate::consent::HookKey::source_hook(name, EVENT, &hook.run),
                label: hook.label,
                optional: hook.optional,
                commit: commit.clone(),
                run_dir: dir.display().to_string(),
                inspect: None,
                browse_url: source.browse_url(&commit),
                replaced: None,
                accepted: accept.items,
            });
        }
    }
    plan
}

/// Whether the managed policy lets `source`'s hooks run: a locked allowlist
/// that no longer matches it does not (POL-12).
fn policy_permits(policy: Option<&crate::policy::Policy>, source: &Source) -> bool {
    !policy.is_some_and(|p| p.lock() && !p.allow_matches(&source.base_identity()))
}

/// Which sources an end-of-command settle considers (HOOK-191).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Only sources whose installed items the command changed.
    Touched,
    /// Those, plus every source matching the filter (`None`: every source)
    /// with a pending items hook: an `upgrade` catches up earlier skips.
    Pending(Option<String>),
}

/// Offer the items hooks the command made pending (HOOK-191): for each source
/// whose installed items differ between `before` and now, and, for an
/// `upgrade`, each source in scope, the pending items hooks are offered once.
/// A source the managed policy's locked allowlist no longer permits is
/// skipped, as its other hooks are (POL-12).
pub fn settle(
    paths: &Paths,
    before: &Snapshot,
    command: &str,
    scope: &Scope,
    accept: crate::hook::Accept,
) -> Result<()> {
    let after = snapshot(paths);
    let registry = Registry::load(paths)?;
    let policy = crate::policy::Policy::load().ok().flatten();
    for source in &registry.sources {
        let touched = before.source(&source.name) != after.source(&source.name);
        let in_scope = match scope {
            Scope::Touched => false,
            Scope::Pending(filter) => filter
                .as_deref()
                .is_none_or(|f| crate::resolve::source_matches_glob(&source.name, f)),
        };
        // spec: HOOK-198 -- a hook the plan listed is offered under its answer,
        // so what the plan showed is what runs.
        let listed = || {
            effective_hooks(paths, source).iter().any(|h| {
                let key = crate::consent::HookKey::source_hook(&source.name, EVENT, &h.run);
                crate::consent::answer_for(&key).is_some()
            })
        };
        if !touched && !in_scope && !listed() {
            continue;
        }
        if !policy_permits(policy.as_ref(), source) {
            continue;
        }
        if effective_hooks(paths, source).is_empty() {
            continue;
        }
        offer(paths, &source.name, command, accept, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, name: &str, hash: &str) -> AppliedItem {
        AppliedItem {
            entry: ItemEntry {
                kind: kind.to_string(),
                name: name.to_string(),
                effective_name: name.to_string(),
                store: format!("/store/{kind}/{name}"),
                profiles: Vec::new(),
            },
            hash: hash.to_string(),
        }
    }

    // spec: HOOK-191 HOOK-193
    #[test]
    fn delta_is_taken_against_the_applied_set() {
        let applied = vec![item("skill", "a", "1"), item("skill", "b", "1")];
        let current = vec![item("skill", "a", "2"), item("tool", "c", "1")];
        let d = delta(Some(&applied), &current);
        let names = |v: &[&ItemEntry]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&d.added), ["c"]);
        assert_eq!(names(&d.updated), ["a"]);
        assert_eq!(names(&d.removed), ["b"]);
        assert_eq!(d.summary(), "1 added, 1 updated, 1 removed");
    }

    // spec: HOOK-195
    #[test]
    fn a_hook_never_applied_sees_every_item_as_added_and_an_empty_set_as_settled() {
        let current = vec![item("skill", "a", "1")];
        let d = delta(None, &current);
        assert_eq!(d.added.len(), 1);
        assert!(d.updated.is_empty() && d.removed.is_empty());
        assert!(delta(None, &[]).is_empty());
        assert!(delta(Some(&current), &current).is_empty());
    }

    // spec: HOOK-191 -- the same name under another kind is another item.
    #[test]
    fn items_are_compared_by_kind_and_effective_name() {
        let applied = vec![item("skill", "x", "1")];
        let current = vec![item("agent", "x", "1")];
        let d = delta(Some(&applied), &current);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
    }

    // spec: HOOK-106 HOOK-196
    #[test]
    fn the_skip_note_quotes_the_identity_in_its_remedy() {
        let note = skip_note("regen", "local/a b/c", "not a terminal");
        assert!(note.contains("(not a terminal)"));
        assert!(note.contains("mind hooks run 'local/a b/c' --event items"));
        assert!(note.contains("stays pending"));
    }
}
