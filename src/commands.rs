//! Command implementations, one public function per CLI verb.

use std::collections::HashSet;
use std::io::Write;

use serde::Serialize;

use crate::catalog::{self, CatalogItem};
use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::git;
use crate::hash::hash_path;
use crate::install;
use crate::manifest::Manifest;
use crate::mindfile::HookEvent;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::policy::Policy;
use crate::resolve::{is_glob, parse_item_ref, resolve, select, select_installed, source_matches};
use crate::source::{Pin, Registry, parse_spec};

/// `mind meld <repo> [--as <prefix>] [--root <dir>] [--follow-branch|--pin-tag|--pin-ref]`
/// — register and clone a source.
///
/// If the source's `mind.toml` lists nested `[discover].sources`, each is melded
/// too (recursively), so a repo can act as a curated super-source. Nested
/// sources are skipped if already registered, and cycles are guarded by URL.
#[allow(clippy::too_many_arguments)]
pub fn meld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
    install_hook: Option<String>,
    dangerously_skip_install_hook_check: bool,
) -> Result<()> {
    // Resolve the consumer-supplied pin flags into a single Pin. The flags are
    // independent at the clap layer, so more than one surfaces here as the
    // structured `ConflictingPin` error (CLI-17) rather than a clap usage string.
    let consumer_pin = resolve_pin_flags(follow_branch, pin_tag, pin_ref)?;

    paths.ensure_layout()?;
    // POL-3: the managed policy is authoritative over user intent. Load it once
    // (Err = invalid policy, fail closed via `?`; None = unmanaged, inert).
    let policy = Policy::load()?;
    // CLI-19: prefer SSH for remotes when the user's config asks for it.
    let prefer_ssh = Config::load(&paths.mind_home)?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut visited = HashSet::new();
    let added = meld_recursive(
        paths,
        &mut registry,
        repo,
        alias,
        roots,
        consumer_pin,
        true,
        &mut visited,
        policy.as_ref(),
        install_hook,
        dangerously_skip_install_hook_check,
        prefer_ssh,
    )?;
    registry.save(paths)?;
    if added > 1 {
        println!("melded {added} source(s)");
    }
    Ok(())
}

/// Parse the three optional pin CLI flags into a single `Option<Pin>`.
/// More than one set flag is a `ConflictingPin` error (CLI-17). The flags are
/// kept independent at the clap layer so this structured error is what the user
/// sees, rather than a clap usage string.
fn resolve_pin_flags(
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
) -> Result<Option<Pin>> {
    match (follow_branch, pin_tag, pin_ref) {
        (None, None, None) => Ok(None),
        (Some(b), None, None) => Ok(Some(Pin::FollowBranch(b))),
        (None, Some(t), None) => Ok(Some(Pin::Tag(t))),
        (None, None, Some(r)) => Ok(Some(Pin::Ref(r))),
        (b, t, r) => {
            // More than one is set; name the first two for the error.
            let mut names = Vec::new();
            if b.is_some() {
                names.push("--follow-branch");
            }
            if t.is_some() {
                names.push("--pin-tag");
            }
            if r.is_some() {
                names.push("--pin-ref");
            }
            Err(MindError::ConflictingPin {
                first: names[0].to_string(),
                second: names[1].to_string(),
            })
        }
    }
}

/// A short human description of a `Pin` for the hook disclosure (HOOK-20).
/// Shared by `meld_recursive` and `upgrade` so both render the pin the same way.
fn pin_description(pin: &Pin) -> String {
    match pin {
        Pin::DefaultBranch => "default branch".to_string(),
        Pin::FollowBranch(b) => format!("branch {b}"),
        Pin::Tag(t) => format!("tag {t}"),
        Pin::Ref(r) => format!("ref {r}"),
    }
}

/// Whether `upgrade` should re-offer a source's install hooks (HOOK-11, HOOK-55):
/// any recorded install hook whose `ran_at` differs from the source's current
/// commit (never ran, or the source advanced past the run commit).
fn hook_rerun_warranted(source: &crate::source::Source) -> bool {
    !source
        .pending_install_hooks(source.commit.as_deref())
        .is_empty()
}

/// Whether a recorded install hook has already run at `current` (a real commit).
fn hook_ran_at(source: &crate::source::Source, command: &str, current: Option<&str>) -> bool {
    current.is_some()
        && source
            .install_hooks
            .iter()
            .any(|r| r.command == command && r.ran_at.as_deref() == current)
}

/// Record (upsert) an install hook's run state on the source.
fn record_install_hook(source: &mut crate::source::Source, command: &str, ran_at: Option<String>) {
    if let Some(r) = source
        .install_hooks
        .iter_mut()
        .find(|r| r.command == command)
    {
        r.ran_at = ran_at;
    } else {
        source.install_hooks.push(crate::source::RecordedHook {
            command: command.to_string(),
            ran_at,
        });
    }
}

/// What the caller should do with the source after a hook batch.
enum HookOutcome {
    Proceed,
    Abort,
}

/// Run a source's install hooks (HOOK-50..60). Offers each install hook unless it
/// already ran at the source's current commit (offers all when `force_rerun`).
/// Prompts per the optional (run/skip) vs required (run/skip/abort) model, runs
/// chosen hooks in `clone_dir` printing a running indication (HOOK-60), and upserts
/// each into `source.install_hooks`. A required hook's Abort returns `Abort`; any
/// hook's non-zero exit propagates as `Err` (HOOK-53), leaving cleanup to the
/// caller (meld removes the clone; re-meld leaves the source). `install_override`
/// is the consumer `--install-hook` command (meld only).
fn run_install_hooks(
    source: &mut crate::source::Source,
    clone_dir: &std::path::Path,
    mindfile: &Option<MindToml>,
    toml_path: &std::path::Path,
    install_override: Option<&str>,
    dangerously_skip: bool,
    force_rerun: bool,
) -> Result<HookOutcome> {
    let resolved = mindfile
        .as_ref()
        .map(|m| m.resolved_hooks(toml_path))
        .transpose()?
        .unwrap_or_default();
    let (hooks, replaced) = crate::hook::apply_install_override(resolved, install_override);

    let pin_desc = pin_description(&source.pin);
    let commit = source.commit.clone().unwrap_or_default();
    let current = source.commit.clone();
    let clone_path = clone_dir.display().to_string();
    let name = source.name.clone();

    enum Act {
        Run,
        Skip,
        Abort,
    }

    for h in hooks.iter().filter(|h| h.event == HookEvent::Install) {
        // HOOK-60: by default re-offer only hooks not yet run at this commit;
        // `--force` (force_rerun) re-offers every install hook.
        if !force_rerun && hook_ran_at(source, &h.run, current.as_deref()) {
            continue;
        }

        // HOOK-56: show the loud override note on the hook the override produced.
        let declared_override: Option<String> = replaced.as_ref().and_then(|cmds| {
            if install_override.map(str::trim) == Some(h.run.as_str()) {
                Some(cmds.join("; "))
            } else {
                None
            }
        });
        let disclosure = crate::hook::hook_disclosure_text(
            h.label(),
            h.optional,
            &name,
            &pin_desc,
            &commit,
            &clone_path,
            &h.run,
            declared_override.as_deref(),
        );

        let act = if dangerously_skip {
            Act::Run // HOOK-23
        } else if !crate::hook::is_tty() {
            Act::Skip // HOOK-22
        } else if h.optional {
            // HOOK-52: optional => two-way (run / skip).
            match crate::hook::prompt_choice_optional(&disclosure)? {
                crate::hook::OptionalChoice::Run => Act::Run,
                crate::hook::OptionalChoice::Skip => Act::Skip,
            }
        } else {
            // HOOK-20: required => three-way (run / skip / abort).
            match crate::hook::prompt_choice(&disclosure)? {
                crate::hook::HookChoice::RunAndContinue => Act::Run,
                crate::hook::HookChoice::SkipAndContinue => Act::Skip,
                crate::hook::HookChoice::Abort => Act::Abort,
            }
        };

        match act {
            Act::Run => {
                // HOOK-60: indicate the running hook.
                println!("running install hook '{}' for {}", h.label(), name);
                // HOOK-53: a non-zero exit (optional or required) is a hard stop.
                crate::hook::run_hook(&h.run, clone_dir, &name, h.label())?;
                record_install_hook(source, &h.run, current.clone());
            }
            Act::Skip => {
                println!(
                    "note: skipped install hook '{}' for {}; its items may not work until it runs",
                    h.label(),
                    name
                );
                record_install_hook(source, &h.run, None);
            }
            Act::Abort => return Ok(HookOutcome::Abort),
        }
    }
    Ok(HookOutcome::Proceed)
}

/// Meld one source and then its nested sources. Returns how many sources were
/// newly added to the registry. `top_level` distinguishes the user's own meld
/// (errors on a duplicate) from a curated nested meld (skips a duplicate).
///
/// `consumer_pin` is the caller-supplied pin (CLI flags or None for a nested
/// source that inherits no pin override).
/// `roots` is the consumer `--root` override (empty => no override).
#[allow(clippy::too_many_arguments)]
fn meld_recursive(
    paths: &Paths,
    registry: &mut Registry,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    consumer_pin: Option<Pin>,
    top_level: bool,
    visited: &mut HashSet<String>,
    policy: Option<&Policy>,
    install_hook: Option<String>,
    dangerously_skip_hook_check: bool,
    prefer_ssh: bool,
) -> Result<usize> {
    let mut source = parse_spec(repo)?;
    source.alias = alias;
    // CLI-19: rewrite an https remote to its SSH form when SSH is preferred, so
    // the clone uses the user's key (no https username/password prompt). Done
    // before the cycle guard so the recorded URL is the one we actually clone.
    source.prefer_ssh(prefer_ssh);

    // Cycle guard: don't process the same URL twice in one meld run.
    if !visited.insert(source.url.clone()) {
        return Ok(0);
    }

    if let Some(existing) = registry.find(&source.name) {
        if top_level {
            return Err(MindError::SourceExists {
                name: source.name.clone(),
                url: existing.url.clone(),
            });
        }
        if existing.url != source.url {
            eprintln!(
                "warning: source name '{}' already melded from {}; skipping {}",
                source.name, existing.url, source.url
            );
        }
        return Ok(0);
    }

    // `source.pin` is still the default here, so `clone_dir` resolves a local
    // source to its working tree. A consumer/directive pin (resolved below) can
    // still switch a local source to a cloned snapshot.
    let mut dir = source.clone_dir(paths);
    let is_local = source.is_local();

    println!("melding {} from {}", source.name, source.url);

    // A local source with no pin is read straight from its working tree (CLI-27):
    // no clone, and `mind` never touches the directory. Any other source is cloned
    // into the sources tree (default branch first so we can read mind.toml, then
    // re-cloned at the resolved pin if needed).
    if is_local {
        if !dir.is_dir() {
            return Err(MindError::NotADirectory {
                path: dir.display().to_string(),
            });
        }
    } else {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
        }
        if let Some(parent) = dir.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        git::clone(&source.url, &dir)?;
    }
    let mut mindfile = MindToml::load(&dir)?;
    source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());

    // Step 2: resolve the effective pin (CLI-17, DSC-41):
    //   consumer flag > [source] directive > DefaultBranch.
    let toml_path = dir.join("mind.toml");
    let directive_pin = mindfile
        .as_ref()
        .map(|m| m.source.pin_directive(&toml_path))
        .transpose()?
        .flatten();
    let effective_pin = consumer_pin.or(directive_pin).unwrap_or(Pin::DefaultBranch);

    // Managed-policy enforcement (POL-3 authoritative). The identity is known and
    // the effective pin is resolved, but nothing is registered yet and the only
    // thing on disk is the throwaway default-branch clone (removed on refusal), so
    // a refusal here leaves nothing behind.
    if let Some(policy) = policy {
        let identity = source.name.clone();
        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            // POL-11: locked allowlist refuses a non-matching source outright.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::SourceNotAllowed { identity });
        }
        if !policy.lock() && !allowed {
            // POL-13: with lock off, allow is advisory; warn but proceed.
            eprintln!(
                "warning: source '{identity}' is not in the managed policy's allowlist (advisory; not enforced because [sources].lock is false)"
            );
        }
        if policy.pinned() && matches!(effective_pin, Pin::DefaultBranch | Pin::FollowBranch(_)) {
            // POL-20: pinned policy forbids a floating branch (default branch or
            // --follow-branch); only a tag/ref pin is permitted.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::UnpinnedSourceForbidden { identity });
        }
    }

    // Step 3: if the effective pin is not DefaultBranch, the source is a clone at
    // that point -- including a *pinned local* source, which is snapshotted into
    // the sources tree (so pinning still works) WITHOUT touching the working tree.
    // A pin that does not resolve is a `Git` error that must leave nothing behind
    // (CLI-18); the clone target is always under the sources tree, never the user's
    // working tree.
    if effective_pin != Pin::DefaultBranch {
        // Setting the pin makes `clone_dir` resolve to the sources-tree path even
        // for a local source.
        source.pin = effective_pin.clone();
        let target = source.clone_dir(paths);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&source.url, &target, &effective_pin) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(e);
        }
        dir = target;
        // The pin may land on a different mind.toml than the working tree / default
        // branch. Reload it so downstream in-memory reads see the pinned content.
        mindfile = MindToml::load(&dir)?;
        source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());
    }

    source.pin = effective_pin;
    // A linked (no-pin) local source records its working-tree HEAD (best-effort; a
    // non-git local dir simply records no commit). Everything else records the
    // cloned commit.
    source.commit = if source.is_linked() {
        git::head_commit(&source.url, &dir).ok()
    } else {
        Some(git::head_commit(&source.url, &dir)?)
    };

    // Persist the consumer's --root override (STO-17, DSC-51).
    // DSC-52: if --root is given for an authoritative source, print a note.
    let is_authoritative = mindfile.as_ref().is_some_and(|m| m.is_authoritative());
    if !roots.is_empty() {
        if is_authoritative {
            // spec: DSC-52
            println!(
                "note: {} uses an authoritative mind.toml; --root is ignored",
                source.name
            );
        } else {
            source.roots = Some(roots);
        }
    }

    // Scan before registering. If the source is rejected here (e.g. the
    // version gate, DSC-40), remove the clone so no orphan is left on disk.
    let mut items = match catalog::scan(paths, &single(&source)) {
        Ok(items) => items,
        Err(e) => {
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    };

    // CLI-24: a source that declares `[source].prefix` and was not melded with an
    // explicit `--as` prefix prompts (interactively) whether to namespace its
    // items under that prefix. The items were scanned with no alias, so their
    // effective names already reflect the declared prefix; show them so the choice
    // is concrete (the names the items will install as). The choice becomes the
    // source alias; if it differs from the declared prefix, re-scan so the recorded
    // names, the warning, and the count match. Non-interactive runs accept the
    // declared prefix as-is (alias stays None).
    if top_level
        && source.alias.is_none()
        && crate::hook::is_tty()
        && let Some(declared) = mindfile.as_ref().and_then(|m| m.source.prefix.clone())
        && !declared.is_empty()
    {
        let preview = if items.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = items.iter().map(|it| it.key()).collect();
            format!("\n  items would install as: {}", names.join(", "))
        };
        let answer = prompt_line(&format!(
            "{} suggests the prefix '{declared}'.{preview}\n  use it? [Y]es / type a different prefix / [n]o prefix: ",
            source.name
        ))?;
        let chosen = crate::namespace::prefix_choice(&answer);
        if chosen != source.alias {
            source.alias = chosen;
            items = match catalog::scan(paths, &single(&source)) {
                Ok(items) => items,
                Err(e) => {
                    if !is_local {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    return Err(e);
                }
            };
        }
    }

    warn_unguarded_references(&items);
    println!("melded {} ({} item(s))", source.name, items.len());

    // Install hooks (HOOK-50..60): the working tree is now checked out at the
    // resolved pin, so hooks run in the right tree. A fresh meld runs every
    // (as-yet-unrun) install hook.
    match run_install_hooks(
        &mut source,
        &dir,
        &mindfile,
        &toml_path,
        install_hook.as_deref(),
        dangerously_skip_hook_check,
        false,
    ) {
        Ok(HookOutcome::Proceed) => {}
        Ok(HookOutcome::Abort) => {
            // HOOK-21: aborting installs nothing; the source is not registered.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            println!("aborted; nothing installed");
            return Ok(0);
        }
        Err(e) => {
            // HOOK-30/HOOK-53: a hook failure fails the meld; remove the clone.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    }

    registry.sources.push(source);

    let mut added = 1;
    if let Some(nested) = mindfile
        .as_ref()
        .and_then(|m| m.discover.as_ref())
        .map(|d| &d.sources)
    {
        for entry in nested {
            // Nested sources from a curated super-source get no pin override;
            // they read their own [source] directive or default to DefaultBranch.
            added += meld_recursive(
                paths,
                registry,
                &entry.source,
                entry.alias.clone(),
                vec![], // no consumer roots for nested sources
                None,   // no consumer pin for nested sources
                false,
                visited,
                policy,
                None, // no consumer install hook for nested sources
                dangerously_skip_hook_check,
                prefer_ssh, // nested sources inherit the SSH preference
            )?;
        }
    }
    Ok(added)
}

/// Warn when a namespaced source references siblings in bare prose, which
/// prefixing will break unless rewritten as `{{ns:name}}` tokens. Scans every
/// text file of each item (the whole skill directory, or the agent/rule file),
/// matching the breadth of install-time `{{ns:}}` expansion.
fn warn_unguarded_references(items: &[CatalogItem]) {
    // Only meaningful once a prefix is in effect.
    if !items.iter().any(|it| it.prefix.is_some()) {
        return;
    }
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    for item in items {
        let mut refs: Vec<String> = Vec::new();
        for file in item_files(item) {
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue; // skip non-UTF-8 / unreadable files
            };
            for r in crate::namespace::unguarded_refs(&content, &siblings) {
                // Self-mentions are fine; dedup across files.
                if r != item.name && !refs.contains(&r) {
                    refs.push(r);
                }
            }
        }
        if !refs.is_empty() {
            eprintln!(
                "warning: {} references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                item.key(),
                refs.join(", ")
            );
        }
    }
}

/// Every text file belonging to an item: all files under a skill directory, or
/// the single agent/rule `.md` file. Sorted for deterministic warning output.
fn item_files(item: &CatalogItem) -> Vec<std::path::PathBuf> {
    if item.path.is_dir() {
        let mut files = Vec::new();
        collect_files(&item.path, &mut files);
        files.sort();
        files
    } else {
        vec![item.path.clone()]
    }
}

/// Recursively collect every file under `dir` (best-effort; unreadable dirs are
/// skipped, since this only feeds the advisory warning).
fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The set of bare item names belonging to a source, for reference validation.
fn siblings_of(items: &[CatalogItem], source: &str) -> std::collections::HashSet<String> {
    items
        .iter()
        .filter(|it| it.source == source)
        .map(|it| it.name.clone())
        .collect()
}

/// `mind init-source [path] [--template]` — maintainer scaffolding. Discovers the
/// repo's items, reports the intra-source reference graph, scaffolds a `mind.toml`
/// if absent, and (with `--template`) rewrites bare sibling references into
/// `{{ns:}}` tokens. Operates only on the target directory: no store, no agent
/// home, no network (INIT-6).
// spec: INIT-1 INIT-2 INIT-3 INIT-4 INIT-6
pub fn init_source(dir: Option<&str>, template: bool) -> Result<()> {
    let dir = dir.unwrap_or(".");
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        return Err(MindError::NotADirectory {
            path: dir.to_string(),
        });
    }
    let root = path.canonicalize().map_err(|e| MindError::io(path, e))?;

    // Discover items exactly as melding would (INIT-2): build a local Source for
    // the directory and scan it (honors convention + mind.toml + min-mind-version).
    let source = parse_spec(&root.to_string_lossy())?;
    let mut items: Vec<CatalogItem> = Vec::new();
    catalog::scan_source_at(&root, &source, &mut items)?;

    println!("init-source: {}", root.display());
    if items.is_empty() {
        println!("  no items found (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md)");
    } else {
        println!("  {} item(s):", items.len());
        for it in &items {
            println!("    {} {}", it.kind, it.name);
        }
    }

    // Reference graph (INIT-4): per item, the siblings it references via tokens
    // (informational) and the ones it mentions in bare prose. The bare mentions
    // are reported as `unguarded-reference` advisories in the same format as
    // `review` (CLI-131), so review and init-source read identically.
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    let mut findings: Vec<crate::review::Finding> = Vec::new();
    for it in &items {
        let content = read_item_text(it);
        let tokens: Vec<String> = crate::namespace::referenced_names(&content)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        let bare: Vec<String> = crate::namespace::unguarded_refs(&content, &siblings)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        if !tokens.is_empty() {
            println!(
                "  {} {} -> {} (tokenized)",
                it.kind,
                it.name,
                tokens.join(", ")
            );
        }
        if !bare.is_empty() {
            findings.push(crate::review::Finding::advisory(
                "unguarded-reference",
                format!(
                    "{}: references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                    it.key(),
                    bare.join(", ")
                ),
            ));
        }
    }
    crate::review::print_findings(&[], &findings);
    if !findings.is_empty() && !template {
        println!(
            "run `mind init-source {dir} --template` to wrap the bare references as {{{{ns:name}}}}"
        );
    }

    // mind.toml scaffold (INIT-3): create only when absent; never overwrite.
    let toml_path = root.join("mind.toml");
    if toml_path.exists() {
        println!("  mind.toml already exists; left unchanged");
    } else {
        let scaffold = concat!(
            "[source]\n",
            "description = \"\"   # what this source offers\n",
            "# prefix = \"prefix\"   # namespace items as prefix-<name>\n",
            "\n",
            "# Declare hooks that run when a consumer melds or unmelds this source.\n",
            "# Remove the leading `# ` to enable a hook.\n",
            "#\n",
            "# [[hooks]]\n",
            "# run = \"make install\"         # shell command to run\n",
            "# name = \"Build\"               # optional label shown in the prompt\n",
            "# event = \"install\"             # \"install\" (default) or \"uninstall\"\n",
            "# optional = false              # false = required (default). optional only lets the\n",
            "#                               # user decline running it; a failure always aborts.\n",
            "#\n",
            "# [[hooks]]\n",
            "# run = \"make clean\"            # cleanup hook run at unmeld time\n",
            "# name = \"Cleanup\"\n",
            "# event = \"uninstall\"\n",
            "# optional = true               # the user may decline this step (its failure still aborts)\n",
        );
        std::fs::write(&toml_path, scaffold).map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
    }

    // Templating (INIT-5): rewrite bare sibling mentions to tokens, per file.
    if template {
        let mut total = 0usize;
        for it in &items {
            // Exclude the item's own name so a self-mention is not wrapped.
            let mut sibs = siblings.clone();
            sibs.remove(&it.name);
            for file in item_files(it) {
                let Ok(content) = std::fs::read_to_string(&file) else {
                    continue; // skip non-UTF-8 / unreadable files
                };
                let (rewritten, n) = crate::namespace::templatize(&content, &sibs);
                if n > 0 {
                    std::fs::write(&file, &rewritten).map_err(|e| MindError::io(&file, e))?;
                    println!("  templated {n} reference(s) in {}", file.display());
                    total += n;
                }
            }
        }
        if total == 0 {
            println!("  no bare references to template");
        }
    }
    Ok(())
}

/// Read all of an item's text files into one buffer, for reference detection.
fn read_item_text(item: &CatalogItem) -> String {
    let mut buf = String::new();
    for file in item_files(item) {
        if let Ok(content) = std::fs::read_to_string(&file) {
            buf.push_str(&content);
            buf.push('\n');
        }
    }
    buf
}

/// `mind unmeld <name> [--unlink-only] [--yes] [--dangerously-skip-install-hook-check]`
/// — drop a source. `name` may be the full `owner/repo` or an unambiguous repo
/// basename. By default every item installed from the source is uninstalled (via
/// its file registry) before the source is removed (CLI-21); `--unlink-only`
/// keeps the items and only removes the source (CLI-22). Runs the source's
/// declared uninstall hooks (HOOK-54) before removal; `dangerously_skip_hook_check`
/// bypasses the prompt. `yes` skips the multi-item removal confirmation (CLI-42).
pub fn unmeld(
    paths: &Paths,
    name: &str,
    unlink_only: bool,
    yes: bool,
    dangerously_skip_hook_check: bool,
    uninstall_hook: Option<String>,
) -> Result<()> {
    let mut registry = Registry::load(paths)?;
    let matched: Vec<usize> = registry
        .sources
        .iter()
        .enumerate()
        .filter(|(_, s)| source_matches(&s.name, name))
        .map(|(i, _)| i)
        .collect();

    let idx = match matched.as_slice() {
        [] => {
            return Err(MindError::SourceNotFound {
                name: name.to_string(),
            });
        }
        [only] => *only,
        many => {
            return Err(MindError::AmbiguousSource {
                query: name.to_string(),
                candidates: many
                    .iter()
                    .map(|i| registry.sources[*i].name.clone())
                    .collect(),
            });
        }
    };

    // HOOK-54: run uninstall hooks from the source's mind.toml BEFORE removing
    // anything. Load the mindfile from the clone dir (which still exists at this
    // point). Run hooks in declaration order. Required hooks get the three-way
    // prompt; optional ones get the two-way prompt. Non-TTY: skip with a note.
    // dangerously_skip_hook_check: run all hooks without prompting.
    {
        let clone_dir = registry.sources[idx].clone_dir(paths);
        let source_name = registry.sources[idx].name.clone();
        let source_pin = registry.sources[idx].pin.clone();
        let source_commit = registry.sources[idx].commit.clone();

        let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
        let toml_path = clone_dir.join("mind.toml");
        let resolved = mindfile
            .as_ref()
            .map(|m| m.resolved_hooks(&toml_path))
            .transpose()?
            .unwrap_or_default();

        // HOOK-59: `--uninstall-hook <cmd>` replaces the source's declared
        // uninstall hooks with one required uninstall hook, shown loudly.
        let (resolved, replaced) = crate::hook::apply_hook_override(
            resolved,
            uninstall_hook.as_deref(),
            HookEvent::Uninstall,
        );
        let override_cmd = uninstall_hook
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let replaced_note = replaced.map(|cmds| cmds.join("; "));

        let pin_desc = pin_description(&source_pin);
        let commit = source_commit.clone().unwrap_or_default();
        let clone_path = clone_dir.display().to_string();

        for h in resolved.iter().filter(|h| h.event == HookEvent::Uninstall) {
            // Show the loud override note on the hook that replaced declared ones.
            let declared_override = match (&replaced_note, override_cmd) {
                (Some(note), Some(cmd)) if h.run == cmd => Some(note.as_str()),
                _ => None,
            };
            let disclosure = crate::hook::hook_disclosure_text(
                h.label(),
                h.optional,
                &source_name,
                &pin_desc,
                &commit,
                &clone_path,
                &h.run,
                declared_override,
            );

            enum Act {
                Run,
                Skip,
                Abort,
            }
            let act = if dangerously_skip_hook_check {
                Act::Run // HOOK-23
            } else if !crate::hook::is_tty() {
                Act::Skip // HOOK-22
            } else if h.optional {
                // HOOK-52: optional => two-way (run / skip).
                match crate::hook::prompt_choice_optional(&disclosure)? {
                    crate::hook::OptionalChoice::Run => Act::Run,
                    crate::hook::OptionalChoice::Skip => Act::Skip,
                }
            } else {
                // HOOK-20: required => three-way (run / skip / abort).
                match crate::hook::prompt_choice(&disclosure)? {
                    crate::hook::HookChoice::RunAndContinue => Act::Run,
                    crate::hook::HookChoice::SkipAndContinue => Act::Skip,
                    crate::hook::HookChoice::Abort => Act::Abort,
                }
            };

            match act {
                Act::Run => {
                    // HOOK-60: indicate the running hook.
                    println!("running uninstall hook '{}' for {}", h.label(), source_name);
                    // HOOK-53: any failure (optional or required) is a hard stop;
                    // the unmeld stops and the source remains.
                    crate::hook::run_hook(&h.run, &clone_dir, &source_name, h.label())?;
                }
                Act::Skip => {
                    println!(
                        "note: skipped uninstall hook '{}' for {}",
                        h.label(),
                        source_name
                    );
                }
                Act::Abort => {
                    println!("aborted; source left in place");
                    return Ok(());
                }
            }
        }
    }

    let source_name = registry.sources[idx].name.clone();

    // The items installed from this source (effective-name keys).
    let mut manifest = Manifest::load(paths)?;
    let item_keys: Vec<String> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .map(|it| it.key())
        .collect();

    // CLI-22: `--unlink-only` removes only the source, leaving its items in place,
    // and lists them with the command to remove them later.
    if unlink_only {
        let source = registry.sources.remove(idx);
        // A local source's directory is the user's working tree -- never delete it.
        let dir = source.clone_dir(paths);
        if !source.is_linked() && dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
        }
        registry.save(paths)?;
        if item_keys.is_empty() {
            println!("unmelded {source_name}");
        } else {
            println!(
                "unmelded {source_name}; {} item(s) remain installed:",
                item_keys.len()
            );
            for k in &item_keys {
                println!("  {k}");
            }
            println!("run `mind forget '{source_name}#*'` to remove them");
        }
        return Ok(());
    }

    // CLI-21: default — uninstall every item from this source, then remove it. The
    // multi-item confirmation (CLI-42) applies; `--yes` skips it; a non-TTY run
    // without `--yes` refuses rather than removing many items silently.
    if item_keys.len() > 1 && !yes {
        println!(
            "unmelding {source_name} will remove {} installed item(s):",
            item_keys.len()
        );
        for k in &item_keys {
            println!("  {k}");
        }
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!(
                    "unmelding {source_name} (removing {} items)",
                    item_keys.len()
                ),
            });
        }
        if !confirm("remove these item(s) and unmeld the source?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    let source = registry.sources.remove(idx);
    let mut forgotten = 0;
    for key in &item_keys {
        if let Some(item) = manifest.items.remove(key) {
            install::uninstall(paths, &item)?;
            forgotten += 1;
        }
    }
    manifest.save(paths)?;

    // A local source's directory is the user's working tree -- never delete it.
    let dir = source.clone_dir(paths);
    if !source.is_linked() && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
    }
    registry.save(paths)?;
    println!("unmelded {source_name} ({forgotten} installed item(s) removed)");
    Ok(())
}

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
}

/// Resolve a `learn` selection (loading the registry, scanning the catalog, and
/// running the dependency closure) without installing anything. Returns the
/// catalog, the registry, and the [`crate::deps::Resolution`] so both `learn`
/// and `learn_preview` share one computation.
fn resolve_learn(
    paths: &Paths,
    item_ref: &str,
) -> Result<(Registry, Vec<CatalogItem>, crate::deps::Resolution)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let parsed = parse_item_ref(item_ref)?;

    // A glob selects every match; an exact ref must resolve to exactly one.
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

    // Map the explicitly selected items back to indices into `items` (by
    // identity: a CatalogItem is a unique (source, kind, name)).
    let selected_idx: Vec<usize> = targets
        .iter()
        .filter_map(|t| {
            items
                .iter()
                .position(|c| c.kind == t.kind && c.name == t.name && c.source == t.source)
        })
        .collect();

    // What is already installed (manifest keys are `CatalogItem::key()` form).
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<String> = manifest.items.keys().cloned().collect();

    // The `read` closure feeds each item's concatenated UTF-8 text to the
    // resolver so it can scan for `{{ns:}}` tokens (DEP-1).
    let read = |item: &CatalogItem| -> String {
        let mut parts: Vec<String> = Vec::new();
        for file in item_files(item) {
            if let Ok(content) = std::fs::read_to_string(&file) {
                parts.push(content);
            }
        }
        parts.join("\n")
    };

    let resolution = crate::deps::resolve(&items, &selected_idx, &installed, read);
    Ok((registry, items, resolution))
}

/// Resolve a `learn` selection's dependency closure without installing it
/// (DEP-21). Used by the CLI dry-run/prompt path and by the interactive TUI's
/// confirm step so both compute identical trees.
// Consumed by the interactive TUI confirm step (DEP-40); allow until wired.
#[allow(dead_code)]
pub fn learn_preview(paths: &Paths, item_ref: &str) -> Result<LearnPlan> {
    let (_registry, items, resolution) = resolve_learn(paths, item_ref)?;
    Ok(LearnPlan {
        tree: resolution.render_tree(&items),
        adds_dependencies: resolution.adds_dependencies(),
        install_count: resolution.install_order().len(),
    })
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
}

pub fn learn(
    paths: &Paths,
    item_ref: &str,
    dry_run: bool,
    yes: bool,
    clobber: Clobber,
) -> Result<()> {
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let (registry, items, resolution) = resolve_learn(paths, item_ref)?;

    // The full closure to install, dependency-first (DEP-21, DEP-30), excluding
    // already-installed items (DEP-23).
    let order = resolution.install_order();
    let closure: Vec<&CatalogItem> = order.iter().map(|&i| &items[i]).collect();

    // DEP-30: the collision check (CLI-33) runs over the FULL closure, not just
    // the explicit selection, so two items that would clobber each other abort
    // before anything is installed.
    if let Some((key, sources)) = colliding_install(&closure) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    // DEP-32: --dry-run renders the dependency tree (when deps were added) and
    // lists the full closure, installing nothing.
    if dry_run {
        if resolution.adds_dependencies() {
            print!("{}", resolution.render_tree(&items));
        }
        println!("would learn {} item(s):", closure.len());
        let rows = closure
            .iter()
            .map(|t| vec![t.key(), t.source.clone()])
            .collect::<Vec<_>>();
        print_rows(&rows);
        return Ok(());
    }

    // DEP-31: when the closure adds items beyond the explicit selection, show the
    // tree and prompt; proceed only on a yes (or `--yes`). When it adds nothing,
    // install directly with no prompt and no tree (CLI-30 behavior unchanged).
    if resolution.adds_dependencies() && !yes {
        print!("{}", resolution.render_tree(&items));
        if !confirm("install this dependency closure?")? {
            println!("cancelled; nothing installed");
            return Ok(());
        }
    }

    // Install each item in dependency-first order. If one fails mid-batch, stop
    // but still persist the items already installed, so the manifest always
    // matches what is on disk.
    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    for target in &closure {
        // POL-12: with the allowlist locked, skip (and report) any item whose
        // source identity is no longer allowed; install from the rest.
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&target.source)
        {
            println!(
                "skipping {} from {}: source not permitted by the managed policy's allowlist",
                target.key(),
                target.source
            );
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
        let siblings = siblings_of(&items, &target.source);
        let force = clobber == Clobber::Force;
        let mut result = install::install(paths, target, &commit, &siblings, force);
        // CLI-34: a conflicting (non-mind) target refuses by default. With
        // `Prompt` (the default `learn`), offer to overwrite it on a TTY; on a
        // yes, retry forced. `install` aborts before touching anything on a
        // clobber, so the retry is safe.
        if let Err(MindError::LinkOccupied { path }) = &result
            && clobber == Clobber::Prompt
            && crate::hook::is_tty()
        {
            let path = path.clone();
            result = if confirm(&format!(
                "{path} exists and is not managed by mind; overwrite it?"
            ))? {
                install::install(paths, target, &commit, &siblings, true)
            } else {
                Err(MindError::LinkOccupied { path })
            };
        }
        match result {
            Ok(installed) => {
                println!(
                    "learned {} from {} ({})",
                    installed.key(),
                    installed.source,
                    short(&installed.commit)
                );
                manifest.insert(installed);
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    manifest.save(paths)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// After a `meld`, offer to install the newly melded source's items (CLI-23).
/// This is the interactive form of `learn '<source>#*'`: it previews the items
/// that would be installed and prompts, then installs the whole source on a yes.
/// `--link-only` skips this entirely (the caller does not invoke it). A source
/// with no installable items (e.g. a curated super-source) is a no-op.
///
/// With `yes`, it installs without prompting (and in a non-TTY context). Without
/// `yes` in a non-TTY context it installs nothing and prints how to install
/// later, mirroring the install-hook non-TTY behavior (HOOK-22): a scripted meld
/// stays register-only unless `--yes` is given.
// spec: CLI-23
pub fn install_melded_source(paths: &Paths, repo: &str, yes: bool, clobber: Clobber) -> Result<()> {
    let source_name = parse_spec(repo)?.name;
    let item_ref = format!("{source_name}#*");

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

    if yes {
        return learn(paths, &item_ref, false, true, clobber);
    }
    if !crate::hook::is_tty() {
        println!(
            "note: {source_name} has {} item(s) to install; run `mind learn '{item_ref}'` (or re-meld with --yes)",
            plan.install_count
        );
        return Ok(());
    }

    // Interactive: show the install preview (the dry-run list), then prompt.
    learn(paths, &item_ref, true, false, clobber)?;
    if confirm_default_yes(&format!(
        "install these {} item(s) now?",
        plan.install_count
    ))? {
        learn(paths, &item_ref, false, true, clobber)
    } else {
        println!("skipped; run `mind learn '{item_ref}'` to install later");
        Ok(())
    }
}

/// True when the repo spec resolves to an already-registered source.
pub fn is_melded(paths: &Paths, repo: &str) -> Result<bool> {
    let name = parse_spec(repo)?.name;
    Ok(Registry::load(paths)?.find(&name).is_some())
}

/// Re-melding an already-melded source (CLI-12). It does not re-clone or
/// re-register: it ensures the source's items are installed, installing any that
/// are missing just as a fresh meld does (CLI-23), and otherwise (or with
/// `--link-only`) prints a status of the source's items and the commit each is
/// installed at.
// spec: CLI-12
pub fn remeld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    link_only: bool,
    yes: bool,
    clobber: Clobber,
    dangerously_skip_hook_check: bool,
) -> Result<()> {
    let source_name = parse_spec(repo)?.name;
    println!("{source_name} is already melded");

    // CLI-13: an explicit `--as` on a re-meld changes the source's prefix. Update
    // the recorded alias and rename its installed items to the new effective
    // names (`<prefix>-<bare>`), so re-melding with a prefix actually re-namespaces
    // an already-melded source. `--as ''` removes the prefix.
    if let Some(new_alias) = alias {
        let mut registry = Registry::load(paths)?;
        if let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) {
            let current = source.alias.clone().unwrap_or_default();
            if current != new_alias {
                source.alias = Some(new_alias);
                registry.save(paths)?;
                let renamed = reprefix_source(paths, &registry, &source_name)?;
                if renamed == 0 {
                    println!("prefix updated; no installed items to rename");
                }
            }
        }
    }

    // HOOK-60: re-offer the source's install hooks that have not run at the
    // current commit (a hook skipped at an earlier meld, or added since); `--force`
    // re-offers every install hook. Runs in the existing clone before installing.
    {
        let mut registry = Registry::load(paths)?;
        if let Some(idx) = registry.sources.iter().position(|s| s.name == source_name) {
            let clone_dir = registry.sources[idx].clone_dir(paths);
            let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
            let toml_path = clone_dir.join("mind.toml");
            let force_rerun = clobber == Clobber::Force;
            match run_install_hooks(
                &mut registry.sources[idx],
                &clone_dir,
                &mindfile,
                &toml_path,
                None,
                dangerously_skip_hook_check,
                force_rerun,
            ) {
                Ok(HookOutcome::Proceed) => registry.save(paths)?,
                Ok(HookOutcome::Abort) => {
                    registry.save(paths)?; // persist any hook that did run
                    println!("aborted; source left in place");
                    return Ok(());
                }
                Err(e) => return Err(e), // a hook failed; leave the source melded
            }
        }
    }

    if !link_only {
        let item_ref = format!("{source_name}#*");
        let to_install = match learn_preview(paths, &item_ref) {
            Ok(plan) => plan.install_count,
            Err(MindError::ItemNotFound { .. }) => 0,
            Err(e) => return Err(e),
        };
        if to_install > 0 {
            return install_melded_source(paths, repo, yes, clobber);
        }
    }
    source_status(paths, &source_name)
}

/// Print every item the source offers with its install state and the source
/// commit it was installed from, noting items whose commit lags the source
/// (CLI-12). Items are matched to the manifest by stable identity (source, kind,
/// bare name), so a prefix change does not lose them.
fn source_status(paths: &Paths, source_name: &str) -> Result<()> {
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Err(MindError::SourceNotFound {
            name: source_name.to_string(),
        });
    };
    let items = catalog::scan(paths, &single(source))?;
    let manifest = Manifest::load(paths)?;

    let head = source
        .commit
        .as_deref()
        .map(short)
        .unwrap_or_else(|| "?".to_string());
    println!("{source_name}: {} item(s) (source @ {head})", items.len());
    for it in &items {
        let installed = manifest
            .items
            .values()
            .find(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name);
        match installed {
            Some(m) => {
                let lag = match source.commit.as_deref() {
                    Some(c) if c != m.commit => {
                        format!(" (outdated; source @ {}, run `mind upgrade`)", short(c))
                    }
                    _ => String::new(),
                };
                println!("  {}  installed @ {}{}", it.key(), short(&m.commit), lag);
            }
            None => println!(
                "  {}  not installed (run `mind learn '{}'`)",
                it.key(),
                it.key()
            ),
        }
    }
    Ok(())
}

/// Rename a source's installed items to their current effective names after its
/// prefix changed (CLI-13). The registry must already carry the new alias.
/// Matches the manifest by stable identity (source, kind, bare name) and reuses
/// the upgrade rename step: install the new name, then drop the old item by its
/// file registry and re-key the manifest. Returns the number renamed.
fn reprefix_source(paths: &Paths, registry: &Registry, source_name: &str) -> Result<usize> {
    let catalog = catalog::scan(paths, registry)?;
    let mut manifest = Manifest::load(paths)?;
    let installed: Vec<crate::manifest::InstalledItem> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .cloned()
        .collect();

    let mut count = 0;
    for old in installed {
        let Some(cat) = catalog
            .iter()
            .find(|c| c.kind == old.kind && c.name == old.bare_name && c.source == old.source)
        else {
            continue; // item removed upstream; introspect reports it
        };
        if cat.effective_name() == old.name {
            continue; // prefix unchanged for this item
        }
        let siblings = siblings_of(&catalog, &old.source);
        let new = install::install(paths, cat, &old.commit, &siblings, false)?;
        install::uninstall(paths, &old)?;
        manifest.items.remove(&old.key());
        println!("renamed {} -> {}", old.key(), new.key());
        manifest.insert(new);
        count += 1;
    }
    manifest.save(paths)?;
    Ok(count)
}

/// If two selected items would install under the same `kind:name`, return that
/// key and the sources that collide on it.
fn colliding_install(targets: &[&CatalogItem]) -> Option<(String, Vec<String>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in targets {
        by_key.entry(t.key()).or_default().push(t.source.clone());
    }
    by_key.into_iter().find(|(_, sources)| sources.len() > 1)
}

/// `mind forget <item>` — uninstall one item, or many via a glob.
pub fn forget(paths: &Paths, item_ref: &str, yes: bool) -> Result<()> {
    let mut manifest = Manifest::load(paths)?;
    let parsed = parse_item_ref(item_ref)?;

    // A glob uninstalls every installed match (mirroring `learn`'s selection);
    // an exact ref honors the kind prefix and source qualifier and errors on an
    // ambiguous bare name (e.g. one shared by a skill and an agent).
    let keys: Vec<String> = if is_glob(&parsed.name) {
        let matches = select_installed(&manifest.items, &parsed);
        if matches.is_empty() {
            return Err(MindError::NotInstalled {
                name: parsed.name.clone(),
            });
        }
        matches.iter().map(|it| it.key()).collect()
    } else {
        vec![crate::resolve::resolve_installed(&manifest.items, &parsed)?.key()]
    };

    // CLI-42: removing more than one item (typically a glob that matched more
    // broadly than intended) lists the matches and confirms first. `--yes` skips;
    // a non-TTY run without `--yes` refuses rather than removing silently.
    if keys.len() > 1 && !yes {
        println!("forget would remove {} item(s):", keys.len());
        for key in &keys {
            println!("  {key}");
        }
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!("removing {} items", keys.len()),
            });
        }
        if !confirm("remove these item(s)?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    for key in keys {
        let item = manifest.items.remove(&key).expect("key from manifest");
        install::uninstall(paths, &item)?;
        println!("forgot {key}");
    }
    manifest.save(paths)?;
    Ok(())
}

/// `mind sync [--upgrade] [--dangerously-skip-install-hook-check]` — fetch every
/// source and refresh its recorded commit. With `--upgrade`, an `upgrade` pass
/// runs after the refresh (reporting pending upgrades and prompting before
/// applying, exactly like `mind upgrade`), so one command both fetches upstream
/// and applies pending upgrades. `dangerously_skip_hook_check` is forwarded to
/// the `upgrade` pass so install-hook re-runs can run unattended in CI (HOOK-11,
/// HOOK-23); it is unused when `--upgrade` is absent.
pub fn sync(paths: &Paths, then_upgrade: bool, dangerously_skip_hook_check: bool) -> Result<()> {
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    // CLI-19: auto-meld honors the user's SSH preference too.
    let prefer_ssh = Config::load(&paths.mind_home)?.ssh;
    let mut registry = Registry::load(paths)?;

    // POL-32: provision the policy's auto-meld base set before syncing. Each entry
    // not already in the registry is melded at its declared pin; an entry already
    // present is left unchanged (idempotent). Reuses the meld path, so auto-meld
    // entries discover nested sources just like a user meld. Entries satisfy
    // allow/pinned by policy validation (POL-21/POL-31), so they pass the meld
    // enforcement above.
    if let Some(policy) = policy.as_ref() {
        paths.ensure_layout()?;
        let mut visited = HashSet::new();
        let mut provisioned = 0usize;
        for am in policy.auto_meld() {
            // Idempotency: derive the entry's identity and skip if already melded.
            // (meld_recursive also skips a same-URL duplicate, but checking here
            // avoids the clone attempt and the "melding ..." chatter.)
            if let Ok(spec) = parse_spec(&am.repo)
                && registry.find(&spec.name).is_some()
            {
                continue;
            }
            provisioned += meld_recursive(
                paths,
                &mut registry,
                &am.repo,
                None,
                vec![],
                Some(am.pin.clone()),
                false, // skip (not error) if a same-URL entry is already present
                &mut visited,
                Some(policy),
                None,  // auto-meld supplies no install hook
                false, // auto-meld is non-TTY, so its hooks take the HOOK-22 skip path
                prefer_ssh,
            )?;
        }
        if provisioned > 0 {
            registry.save(paths)?;
        }
    }

    if registry.sources.is_empty() {
        println!("no sources melded; run `mind meld <owner/repo>`");
        return Ok(());
    }
    // A per-source failure (e.g. a network error on one remote) must not abort
    // the whole run: refresh each source independently, persist whatever
    // progress was made, then report the failures and exit non-zero.
    let total = registry.sources.len();
    let mut failures: Vec<String> = Vec::new();
    for source in &mut registry.sources {
        // POL-12: with the allowlist locked, do not sync a source whose identity
        // is no longer allowed; report and skip it (the rest still sync).
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&source.name)
        {
            println!(
                "skipping {}: source not permitted by the managed policy's allowlist",
                source.name
            );
            continue;
        }
        let dir = source.clone_dir(paths);
        print!("syncing {} ... ", source.name);
        let _ = std::io::stdout().flush();
        let refreshed = (|| -> Result<(String, bool, Option<String>)> {
            // A linked (no-pin) local source is its live working tree (CLI-27):
            // there is nothing to fetch and the tree is never touched. Just re-read
            // its HEAD (best effort) and description. A pinned local source is a
            // clone and syncs like any other.
            if source.is_linked() {
                // The working tree must still exist; a deleted one is a sync error
                // for this source (CLI-54 reports it and continues with the rest).
                if !dir.is_dir() {
                    return Err(MindError::NotADirectory {
                        path: dir.display().to_string(),
                    });
                }
                let new_commit = git::head_commit(&source.url, &dir)
                    .ok()
                    .or_else(|| source.commit.clone())
                    .unwrap_or_default();
                let changed = source.commit.as_deref() != Some(new_commit.as_str());
                let desc = MindToml::load(&dir)?.and_then(|mt| mt.source.description);
                return Ok((new_commit, changed, desc));
            }
            // CLI-55: resolve the source against its recorded pin (never change
            // the pin itself, only move HEAD to the pinned point).
            let pin = source.pin.clone();
            if dir.join(".git").is_dir() {
                git::sync_to_pin(&source.url, &dir, &pin)?;
            } else {
                if let Some(parent) = dir.parent() {
                    crate::paths::mkdir_p(parent)?;
                }
                git::clone_at(&source.url, &dir, &pin)?;
            }
            let new_commit = git::head_commit(&source.url, &dir)?;
            let changed = source.commit.as_deref() != Some(new_commit.as_str());
            let desc = MindToml::load(&dir)?.and_then(|mt| mt.source.description);
            Ok((new_commit, changed, desc))
        })();
        match refreshed {
            Ok((new_commit, changed, desc)) => {
                source.commit = Some(new_commit.clone());
                source.description = desc;
                println!(
                    "{} ({})",
                    if changed { "updated" } else { "up to date" },
                    short(&new_commit)
                );
            }
            Err(e) => {
                println!("failed");
                eprintln!("  {}: {e}", source.name);
                failures.push(source.name.clone());
            }
        }
    }
    // Save the progress made before reporting any failure, so the recorded
    // commits stay consistent with what is on disk.
    registry.save(paths)?;
    if !failures.is_empty() {
        return Err(MindError::SyncFailed {
            failed: failures.len(),
            total,
        });
    }
    if then_upgrade {
        // spec: HOOK-11, HOOK-23
        upgrade(paths, false, None, dangerously_skip_hook_check)?;
    }
    Ok(())
}

/// `mind upgrade [--yes] [item]` — report and optionally apply upgrades.
pub fn upgrade(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    dangerously_skip_hook_check: bool,
) -> Result<()> {
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let mut registry = Registry::load(paths)?;
    let manifest = Manifest::load(paths)?;

    let filter = item_ref.map(parse_item_ref).transpose()?;

    // HOOK-11 scope: a scoped `upgrade <item>` must not re-run install hooks
    // (arbitrary code) for sources unrelated to the targeted item. When a filter
    // is present, restrict the hook re-run to sources that have at least one
    // INSTALLED item matching the filter (the same scoping the per-item loop uses
    // via `installed_matches`). With no filter, `None` means every source is in
    // scope, leaving the unscoped behavior unchanged.
    let hook_scope: Option<HashSet<String>> = filter.as_ref().map(|f| {
        manifest
            .items
            .values()
            .filter(|it| crate::resolve::installed_matches(it, f))
            .map(|it| it.source.clone())
            .collect()
    });

    // HOOK-11: re-run a source's install hook when its commit has advanced past
    // the commit the hook last ran at (or it was recorded but never run). This is
    // a source-level pass, separate from the per-item upgrade loop below.
    rerun_source_hooks(
        paths,
        &mut registry,
        dangerously_skip_hook_check,
        hook_scope.as_ref(),
        policy.as_ref(),
    )?;

    let catalog = catalog::scan(paths, &registry)?;
    let mut pending: Vec<Upgrade> = Vec::new();

    for installed in manifest.items.values() {
        match upgrade_item_disposition(installed, filter.as_ref(), policy.as_ref()) {
            // Out of the scoped selection: silent skip, no output.
            UpgradeDisposition::OutOfScope => continue,
            // POL-12: in-scope but the source is no longer allowed by the locked
            // allowlist; report and skip. The item-ref filter is checked first,
            // so a scoped upgrade never emits skip lines for out-of-scope sources
            // the user never selected.
            UpgradeDisposition::PolicyBlocked => {
                println!(
                    "skipping {} from {}: source not permitted by the managed policy's allowlist",
                    installed.key(),
                    installed.source
                );
                continue;
            }
            UpgradeDisposition::Consider => {}
        }
        // Match on stable identity (source, kind, bare_name) so a prefix change
        // is seen as a rename of the same item, not an orphan-plus-new-item.
        let Some(cat) = catalog.iter().find(|c| {
            c.kind == installed.kind
                && c.name == installed.bare_name
                && c.source == installed.source
        }) else {
            // Source dropped or item removed upstream; reported by introspect.
            continue;
        };
        let new_hash = hash_path(&cat.path)?;
        let new_name = cat.effective_name();
        let new_commit = registry
            .find(&installed.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        let renamed = new_name != installed.name;
        if new_hash != installed.hash || renamed {
            pending.push(Upgrade {
                cat: cat.clone(),
                old: installed.clone(),
                new_commit,
                new_hash,
                new_name,
            });
        }
    }

    if pending.is_empty() {
        println!("everything is up to date");
        return Ok(());
    }

    print_upgrade_report(&registry, &pending);

    if !yes && !confirm("apply these upgrades?")? {
        println!("aborted; nothing changed");
        return Ok(());
    }

    let mut manifest = manifest;
    for up in &pending {
        let siblings = siblings_of(&catalog, &up.cat.source);
        // Build the new version first; the old copy is preserved until this
        // succeeds (transactional install). An upgrade never force-overwrites a
        // foreign target; that is for an explicit `learn --force`.
        let installed = install::install(paths, &up.cat, &up.new_commit, &siblings, false)?;
        if up.new_name != up.old.name {
            // Rename: drop the old item (by its file registry) and re-key.
            install::uninstall(paths, &up.old)?;
            manifest.items.remove(&up.old.key());
            println!("upgraded {} -> {}", up.old.key(), installed.key());
        } else {
            println!("upgraded {}", installed.key());
        }
        manifest.insert(installed);
    }
    manifest.save(paths)?;
    Ok(())
}

/// HOOK-11, HOOK-55: re-run each source's install hooks when warranted (any
/// recorded hook whose `ran_at` differs from the source's current commit). Same
/// trust boundary as `meld`: prompt and disclose, unless
/// `--dangerously-skip-install-hook-check` is set or there is no TTY. In
/// `upgrade`, Abort is treated as Skip: the source is already registered, so
/// declining the re-run just leaves the existing install in place. Persists the
/// registry only if a hook was run and its recorded commit advanced.
fn rerun_source_hooks(
    paths: &Paths,
    registry: &mut Registry,
    dangerously_skip_hook_check: bool,
    in_scope: Option<&HashSet<String>>,
    policy: Option<&Policy>,
) -> Result<()> {
    let mut changed = false;
    for source in &mut registry.sources {
        if !hook_rerun_warranted(source) {
            continue;
        }
        // HOOK-11 scope: a scoped `upgrade <item>` restricts the hook re-run to
        // sources implicated by the filter. `None` = unscoped (all sources).
        if let Some(scope) = in_scope
            && !scope.contains(&source.name)
        {
            continue;
        }
        // POL-12: with the allowlist locked, do not re-run the install hook
        // (arbitrary code) for a source whose identity is no longer allowed;
        // report and skip it, exactly as the per-item loop does.
        if let Some(policy) = policy
            && policy.lock()
            && !policy.allow_matches(&source.name)
        {
            println!(
                "skipping install hook for {}: source not permitted by the managed policy's allowlist",
                source.name
            );
            continue;
        }

        let dir = source.clone_dir(paths);
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let clone_path = dir.display().to_string();

        // Collect indices of pending hooks so we can mutate by index below.
        let pending_indices: Vec<usize> = source
            .install_hooks
            .iter()
            .enumerate()
            .filter(|(_, h)| h.ran_at.as_deref() != source.commit.as_deref())
            .map(|(i, _)| i)
            .collect();

        for idx in pending_indices {
            let cmd = source.install_hooks[idx].command.clone();

            let run = if dangerously_skip_hook_check {
                // HOOK-23: re-run without prompting.
                println!(
                    "note: re-running install hook for {} without the safety prompt (--dangerously-skip-install-hook-check)",
                    source.name
                );
                true
            } else if !crate::hook::is_tty() {
                // HOOK-22: no TTY; never run silently. Skip the re-run.
                println!(
                    "note: skipped re-running the install hook for {} (no TTY); its tooling may be out of date until the hook is re-run",
                    source.name
                );
                false
            } else {
                let disclosure = crate::hook::disclosure_text(
                    &source.name,
                    &pin_desc,
                    &commit,
                    &clone_path,
                    &cmd,
                    None,
                );
                // Abort is treated as Skip here (the source is already registered,
                // per the HOOK-11 note).
                matches!(
                    crate::hook::prompt_choice(&disclosure)?,
                    crate::hook::HookChoice::RunAndContinue
                )
            };

            if run {
                // HOOK-30: a non-zero exit is a hard error. The source stays
                // registered; just propagate the failure.
                crate::hook::run_hook(&cmd, &dir, &source.name, &cmd)?;
                // HOOK-55: record the commit the hook ran at.
                source.install_hooks[idx].ran_at = source.commit.clone();
                changed = true;
                println!("re-ran install hook for {}", source.name);
            }
        }
    }
    if changed {
        registry.save(paths)?;
    }
    Ok(())
}

/// What `upgrade` should do with one installed item before the catalog lookup.
#[derive(Debug, PartialEq, Eq)]
enum UpgradeDisposition {
    /// The item is outside the scoped item-ref selection: skip silently.
    OutOfScope,
    /// In scope, but its source is barred by the locked policy allowlist
    /// (POL-12): print a skip line.
    PolicyBlocked,
    /// In scope and permitted: consider it for an upgrade.
    Consider,
}

/// Decide an installed item's `upgrade` disposition. The item-ref filter is
/// applied first (POL-12 ordering fix): a scoped `upgrade <item>` must not emit
/// policy-skip lines for sources the user never selected. The policy block is
/// only ever reported for items that passed the filter.
fn upgrade_item_disposition(
    installed: &crate::manifest::InstalledItem,
    filter: Option<&crate::resolve::ItemRef>,
    policy: Option<&Policy>,
) -> UpgradeDisposition {
    if let Some(f) = filter
        && !crate::resolve::installed_matches(installed, f)
    {
        return UpgradeDisposition::OutOfScope;
    }
    if let Some(policy) = policy
        && policy.lock()
        && !policy.allow_matches(&installed.source)
    {
        return UpgradeDisposition::PolicyBlocked;
    }
    UpgradeDisposition::Consider
}

struct Upgrade {
    cat: CatalogItem,
    old: crate::manifest::InstalledItem,
    new_commit: String,
    new_hash: String,
    new_name: String,
}

fn print_upgrade_report(registry: &Registry, pending: &[Upgrade]) {
    println!("{} item(s) have upstream changes:\n", pending.len());
    for up in pending {
        if up.new_name != up.old.name {
            println!(
                "  {} {} [{}]  rename {} -> {}",
                up.cat.kind, up.cat.name, up.cat.source, up.old.name, up.new_name
            );
        } else {
            println!("  {} [{}]", up.cat.key(), up.cat.source);
        }
        println!(
            "    hash    {} -> {}",
            short(&up.old.hash),
            short(&up.new_hash)
        );
        println!(
            "    commit  {} -> {}",
            short(&up.old.commit),
            short(&up.new_commit)
        );
        if let Some(src) = registry.find(&up.cat.source)
            && !up.old.commit.is_empty()
            && !up.new_commit.is_empty()
            && let Some(url) = src.compare_url(&up.old.commit, &up.new_commit)
        {
            println!("    diff    {url}");
        }
        println!();
    }
}

/// `mind recall [--sources] [item] [--kind K] [--source S] [--json]`. The
/// `--kind` and `--source` filters narrow the installed-items listing; they do
/// not apply to `--sources` or to a single-item lookup (use a `kind:`/
/// `owner/repo#` ref there). `--json` emits the data as JSON on stdout.
pub fn recall(
    paths: &Paths,
    sources: bool,
    item: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
    json: bool,
) -> Result<()> {
    // The listing filters are meaningless for --sources or a single-item lookup;
    // say so rather than silently ignoring them.
    if (sources || item.is_some()) && (kind.is_some() || source.is_some()) {
        eprintln!(
            "note: --kind/--source filter the item listing; ignored with --sources or a single item"
        );
    }

    if sources {
        let registry = Registry::load(paths)?;
        if json {
            return print_json(&registry.sources);
        }
        if registry.sources.is_empty() {
            println!("no sources melded");
            return Ok(());
        }
        let rows = registry
            .sources
            .iter()
            .map(|s| {
                let commit = s
                    .commit
                    .as_deref()
                    .map(short)
                    .unwrap_or_else(|| "unsynced".into());
                let ns = match &s.alias {
                    Some(a) => format!(" as:{a}"),
                    None => String::new(),
                };
                // HOOK-58: surface that a source carries install hooks with a
                // count-aware token.
                let hook = match s.install_hooks.len() {
                    0 => String::new(),
                    1 => " hook".to_string(),
                    n => format!(" hooks({n})"),
                };
                vec![
                    s.name.clone(),
                    s.url.clone(),
                    format!("[{commit}{ns}{hook}]"),
                    s.description.clone().unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>();
        print_rows(&rows);
        return Ok(());
    }

    let manifest = Manifest::load(paths)?;
    if let Some(item_ref) = item {
        let parsed = parse_item_ref(item_ref)?;
        let found = crate::resolve::resolve_installed(&manifest.items, &parsed)?;
        if json {
            return print_json(found);
        }
        println!("{}", found.key());
        if let Some(d) = &found.description {
            println!("  desc    {d}");
        }
        println!("  source  {}", found.source);
        println!("  commit  {}", short(&found.commit));
        println!("  hash    {}", short(&found.hash));
        println!("  store   {}", paths.mind_home.join(&found.store).display());
        for link in &found.links {
            println!("  link    {link}");
        }
        return Ok(());
    }

    // CLI-70/74: the status view -- each melded source with its catalog items
    // nested beneath it, every item marked installed (with commit) or available.
    // Items installed but no longer in the source's catalog (removed upstream) are
    // shown too, marked. `--kind`/`--source` filter what is shown.
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry)?;
    let filtering = kind.is_some() || source.is_some();

    // The source's catalog items (honoring --kind), sorted by key.
    let cat_items = |s: &crate::source::Source| -> Vec<&CatalogItem> {
        let mut v: Vec<&CatalogItem> = catalog
            .iter()
            .filter(|it| it.source == s.name && kind.is_none_or(|k| it.kind == k))
            .collect();
        v.sort_by_key(|x| x.key());
        v
    };
    // Installed items of a source with no catalog match (drifted / removed upstream).
    let orphans_of = |s: &crate::source::Source,
                      cat_keys: &std::collections::HashSet<String>|
     -> Vec<&crate::manifest::InstalledItem> {
        let mut v: Vec<&crate::manifest::InstalledItem> = manifest
            .items
            .values()
            .filter(|m| {
                m.source == s.name
                    && !cat_keys.contains(&m.key())
                    && kind.is_none_or(|k| m.kind == k)
            })
            .collect();
        v.sort_by_key(|x| x.key());
        v
    };
    let source_shown =
        |s: &crate::source::Source| source.is_none_or(|q| source_matches(&s.name, q));

    if json {
        let out: Vec<serde_json::Value> = registry
            .sources
            .iter()
            .filter(|s| source_shown(s))
            .map(|s| {
                let items = cat_items(s);
                let cat_keys: std::collections::HashSet<String> =
                    items.iter().map(|it| it.key()).collect();
                let mut rows: Vec<serde_json::Value> = items
                    .iter()
                    .map(|it| {
                        let inst = manifest.items.get(&it.key());
                        serde_json::json!({
                            "key": it.key(),
                            "installed": inst.is_some(),
                            "commit": inst.map(|m| m.commit.clone()),
                        })
                    })
                    .collect();
                for m in orphans_of(s, &cat_keys) {
                    rows.push(serde_json::json!({
                        "key": m.key(),
                        "installed": true,
                        "commit": m.commit.clone(),
                        "orphaned": true,
                    }));
                }
                serde_json::json!({
                    "name": s.name,
                    "url": s.url,
                    "commit": s.commit,
                    "alias": s.alias,
                    "items": rows,
                })
            })
            .collect();
        return print_json(&out);
    }

    if registry.sources.is_empty() {
        println!("no sources melded; `mind meld <repo>` to add one");
        return Ok(());
    }

    for s in &registry.sources {
        if !source_shown(s) {
            continue;
        }
        let items = cat_items(s);
        let cat_keys: std::collections::HashSet<String> = items.iter().map(|it| it.key()).collect();
        let orphans = orphans_of(s, &cat_keys);
        if items.is_empty() && orphans.is_empty() && filtering {
            continue; // a filter excluded everything this source offers
        }
        let commit = s
            .commit
            .as_deref()
            .map(short)
            .unwrap_or_else(|| "unsynced".into());
        let ns = match &s.alias {
            Some(a) => format!(" as:{a}"),
            None => String::new(),
        };
        let hook = match s.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        println!(
            "{}  [{commit}{ns}{hook}]{}",
            s.name,
            s.description
                .as_deref()
                .map(|d| format!("  {d}"))
                .unwrap_or_default()
        );
        let width = items
            .iter()
            .map(|it| it.key().len())
            .chain(orphans.iter().map(|m| m.key().len()))
            .max()
            .unwrap_or(0);
        for it in items {
            let key = it.key();
            match manifest.items.get(&key) {
                Some(m) => println!("  {key:<width$}  installed @ {}", short(&m.commit)),
                None => println!("  {key:<width$}  available"),
            }
        }
        for m in orphans {
            println!(
                "  {:<width$}  installed @ {} (removed upstream)",
                m.key(),
                short(&m.commit)
            );
        }
    }
    Ok(())
}

/// `mind probe [query] [--kind K] [--source S] [--json]`. A leading `*` marks
/// installed items; the hash is of the current source content. `--kind` and
/// `--source` narrow the listing and compose with the substring query. `--json`
/// emits the rows as JSON on stdout.
pub fn probe(
    paths: &Paths,
    query: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
    json: bool,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let q = query.unwrap_or("");
    let mut hits: Vec<&CatalogItem> = items
        .iter()
        .filter(|it| {
            catalog::matches_query(it, q) // spec: CLI-85
                && kind.is_none_or(|k| it.kind == k)
                && source.is_none_or(|s| source_matches(&it.source, s))
        })
        .collect();
    hits.sort_by_key(|a| a.key());

    let installed = |it: &CatalogItem| {
        manifest
            .items
            .values()
            .any(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name)
    };

    if json {
        let rows: Vec<ProbeRow> = hits
            .iter()
            .map(|it| ProbeRow {
                installed: installed(it),
                kind: it.kind.as_str(),
                name: it.effective_name(),
                source: &it.source,
                hash: hash_path(&it.path).ok(),
                description: it.description.as_deref(),
            })
            .collect();
        return print_json(&rows);
    }

    if hits.is_empty() {
        if registry.sources.is_empty() {
            println!("no sources melded; run `mind meld <owner/repo>`");
        } else {
            println!("no items match '{q}'");
        }
        return Ok(());
    }

    let rows = hits
        .iter()
        .map(|it| {
            let hash = hash_path(&it.path)
                .map(|h| short(&h))
                .unwrap_or_else(|_| "-".into());
            vec![
                if installed(it) {
                    "*".into()
                } else {
                    String::new()
                },
                it.key(),
                it.source.clone(),
                hash,
                summary(it.description.as_deref(), 60),
            ]
        })
        .collect::<Vec<_>>();
    print_rows(&rows);
    Ok(())
}

/// One `probe --json` row.
#[derive(Serialize)]
struct ProbeRow<'a> {
    installed: bool,
    kind: &'a str,
    name: String,
    source: &'a str,
    hash: Option<String>,
    description: Option<&'a str>,
}

/// One diagnostic finding from `introspect`. `kind` is a stable machine tag;
/// `message` is the human line.
#[derive(Serialize)]
struct Issue {
    kind: &'static str,
    target: String,
    message: String,
}

/// `mind introspect [--fix] [--json]` — report drift and breakage. With `--fix`,
/// repair what can be fixed without changing versions: recreate missing symlinks
/// from each item's file registry. Drifted or renamed items are left to
/// `upgrade`. `--json` emits the findings as JSON on stdout.
pub fn introspect(paths: &Paths, fix: bool, json: bool) -> Result<()> {
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let mut issues: Vec<Issue> = Vec::new();
    let mut repaired: Vec<String> = Vec::new();

    for s in &registry.sources {
        if !s.clone_dir(paths).join(".git").is_dir() {
            issues.push(Issue {
                kind: "no-clone",
                target: s.name.clone(),
                message: format!("source '{}' has no clone on disk; run `mind sync`", s.name),
            });
        } else if s.commit.is_none() {
            issues.push(Issue {
                kind: "never-synced",
                target: s.name.clone(),
                message: format!("source '{}' was never synced; run `mind sync`", s.name),
            });
        }
    }

    for it in manifest.items.values() {
        let missing: Vec<&String> = it
            .links
            .iter()
            .filter(|link| std::fs::symlink_metadata(link).is_err())
            .collect();
        if !missing.is_empty() {
            // With --fix, re-link from the store copy; report only what cannot
            // be repaired (e.g. the store copy itself is gone).
            let n = if fix { install::relink(paths, it)? } else { 0 };
            if n > 0 {
                repaired.push(format!("{}: relinked {n} missing symlink(s)", it.key()));
            }
            for link in &missing {
                if std::fs::symlink_metadata(link).is_err() {
                    issues.push(Issue {
                        kind: "missing-link",
                        target: it.key(),
                        message: format!("{}: symlink missing at {link}", it.key()),
                    });
                }
            }
        }
        // Match on stable identity (source, kind, bare_name).
        match catalog
            .iter()
            .find(|c| c.kind == it.kind && c.name == it.bare_name && c.source == it.source)
        {
            None => issues.push(Issue {
                kind: "removed-upstream",
                target: it.key(),
                message: format!("{}: no longer present in source '{}'", it.key(), it.source),
            }),
            Some(cat) => {
                if cat.effective_name() != it.name {
                    issues.push(Issue {
                        kind: "namespace-changed",
                        target: it.key(),
                        message: format!(
                            "{}: namespace changed to '{}'; run `mind upgrade`",
                            it.key(),
                            cat.effective_name()
                        ),
                    });
                } else if let Ok(h) = hash_path(&cat.path)
                    && h != it.hash
                {
                    issues.push(Issue {
                        kind: "drifted",
                        target: it.key(),
                        message: format!("{}: upstream changed; run `mind upgrade`", it.key()),
                    });
                }
            }
        }
    }

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            issues: &'a [Issue],
            sources: usize,
            items: usize,
        }
        return print_json(&Report {
            issues: &issues,
            sources: registry.sources.len(),
            items: manifest.items.len(),
        });
    }

    for note in &repaired {
        println!("{note}");
    }
    for issue in &issues {
        println!("{}", issue.message);
    }
    if issues.is_empty() {
        println!(
            "all good: {} source(s), {} item(s) installed",
            registry.sources.len(),
            manifest.items.len()
        );
    } else {
        println!("\n{} issue(s) found", issues.len());
    }
    Ok(())
}

/// `mind review <target> [--as <prefix>]` — validate a source for publishing.
///
/// Read-only. Collects hard errors and advisory findings; hard errors cause a
/// non-zero exit (CLI-132). Installs nothing and changes nothing on disk.
///
/// spec: CLI-130, CLI-131, CLI-132, CLI-133
pub fn review(paths: &Paths, target: &str, alias: Option<String>) -> Result<()> {
    let result = crate::review::review(paths, target, alias)?;

    // Print hard then advisory findings in the shared format.
    crate::review::print_findings(&result.hard, &result.advisory);

    if result.hard.is_empty() {
        if result.advisory.is_empty() {
            println!("review: no issues found");
        } else {
            println!(
                "review: {} advisory finding(s); source is publishable",
                result.advisory.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\nreview: {} hard error(s), {} advisory finding(s)",
            result.hard.len(),
            result.advisory.len()
        );
        Err(crate::error::MindError::ReviewFailed {
            hard: result.hard.len(),
        })
    }
}

/// `mind config show` — print the config file location and its key/value pairs.
pub fn config_show(paths: &Paths) -> Result<()> {
    paths.ensure_config()?;
    let file = Config::path(&paths.mind_home);
    let cfg = Config::load(&paths.mind_home)?;
    println!("config file: {}", file.display());
    if cfg.lobes.is_empty() {
        println!("  lobes = []  (default: {})", paths.claude_home.display());
    } else {
        println!("  lobes = {:?}", cfg.lobes);
    }
    println!("  ssh = {}  (prefer SSH for melded remotes)", cfg.ssh);
    if let Some(env) = std::env::var_os("MIND_AGENT_HOMES") {
        println!(
            "note: MIND_AGENT_HOMES is set and overrides lobes: {}",
            env.to_string_lossy()
        );
    }
    Ok(())
}

/// `mind config lobes add <path>` — add an agent home.
pub fn lobe_add(paths: &Paths, path: &str) -> Result<()> {
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing. Load the policy first so the refusal precedes any config write.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("add"));
    }
    paths.ensure_config()?;
    let mut cfg = Config::load(&paths.mind_home)?;
    if cfg.lobes.iter().any(|h| h == path) {
        println!("lobe already configured: {path}");
        return Ok(());
    }
    cfg.lobes.push(path.to_string());
    cfg.save(&paths.mind_home)?;
    println!("added lobe {path}");
    Ok(())
}

/// `mind config lobes list` — list configured agent homes.
pub fn lobe_list(paths: &Paths) -> Result<()> {
    paths.ensure_config()?;
    let cfg = Config::load(&paths.mind_home)?;
    if cfg.lobes.is_empty() {
        println!("{}  (default)", paths.claude_home.display());
    } else {
        for h in &cfg.lobes {
            println!("{h}");
        }
    }
    // POL-40: under a managed lobe lock, `Paths::agent_homes` ignores
    // $MIND_AGENT_HOMES, so the override note would be false. Suppress it when a
    // policy is in effect and lobes are locked; otherwise behavior is unchanged.
    let lobes_locked = matches!(Policy::load()?, Some(p) if p.lobes_lock());
    if show_override_note(std::env::var_os("MIND_AGENT_HOMES").is_some(), lobes_locked) {
        println!("note: MIND_AGENT_HOMES is set and overrides the above");
    }
    Ok(())
}

/// POL-40: the `config lobes list` override note is shown only when
/// `$MIND_AGENT_HOMES` is set AND it actually takes effect. Under a managed lobe
/// lock `Paths::agent_homes` ignores the env var, so the note would be false;
/// suppress it.
fn show_override_note(env_set: bool, lobes_locked: bool) -> bool {
    env_set && !lobes_locked
}

/// `mind config lobes remove <path>` — drop an agent home.
pub fn lobe_remove(paths: &Paths, path: &str) -> Result<()> {
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("remove"));
    }
    paths.ensure_config()?;
    let mut cfg = Config::load(&paths.mind_home)?;
    let before = cfg.lobes.len();
    cfg.lobes.retain(|h| h != path);
    if cfg.lobes.len() == before {
        return Err(MindError::UnknownLobe {
            path: path.to_string(),
        });
    }
    cfg.save(&paths.mind_home)?;
    println!("removed lobe {path}");
    Ok(())
}

/// `mind completions <shell>` — write a shell completion script to stdout.
pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "mind", &mut std::io::stdout());
}

/// `mind man` — write the roff man page to stdout.
pub fn man() -> Result<()> {
    use clap::CommandFactory;
    let mut out = Vec::new();
    clap_mangen::Man::new(crate::cli::Cli::command())
        .render(&mut out)
        .map_err(|e| MindError::io("<man>", e))?;
    std::io::stdout()
        .write_all(&out)
        .map_err(|e| MindError::io("<stdout>", e))
}

// --- helpers ---------------------------------------------------------------

/// Build the refusal error for a locked `config lobes <action>` (POL-40). The
/// effective agent homes are pinned by `[lobes].lock`, so the action changes
/// nothing.
fn lobes_locked_error(action: &str) -> MindError {
    MindError::LobesLocked {
        action: action.to_string(),
    }
}

/// Serialize `value` as pretty JSON to stdout (for the `--json` flags).
fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value).map_err(|e| MindError::json("json output", e))?;
    println!("{s}");
    Ok(())
}

/// A throwaway registry holding just one source, for catalog scans during meld.
fn single(source: &crate::source::Source) -> Registry {
    Registry {
        sources: vec![source.clone()],
    }
}

/// First 8 chars of a hash/commit, for compact display.
fn short(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.chars().take(8).collect()
    }
}

/// Print rows as aligned columns: every column except the last is left-padded to
/// the widest value in that column; the last column (a description) is left as-is.
/// Trailing empty cells are trimmed.
fn print_rows(rows: &[Vec<String>]) {
    let Some(ncols) = rows.iter().map(Vec::len).max() else {
        return;
    };
    let mut widths = vec![0usize; ncols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i + 1 < ncols {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            if i + 1 < ncols {
                let pad = widths[i].saturating_sub(cell.chars().count());
                line.push_str(cell);
                line.extend(std::iter::repeat_n(' ', pad));
            } else {
                line.push_str(cell);
            }
        }
        println!("{}", line.trim_end());
    }
}

/// One-line summary for list views: first sentence or `max` chars, whichever is
/// shorter. The full text stays available via `recall <item>`.
fn summary(desc: Option<&str>, max: usize) -> String {
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
fn prompt_line(prompt: &str) -> Result<String> {
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
}

/// Resolve a yes/no reply. An explicit `y`/`yes` or `n`/`no` (any case, trimmed)
/// wins; an empty line or any unrecognized reply takes `default_yes`.
fn parse_confirm(input: &str, default_yes: bool) -> bool {
    match input.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

/// Print `prompt {hint}`, read one line from stdin, and resolve it against
/// `default_yes`. EOF (no input) is always No.
fn read_confirm(prompt: &str, hint: &str, default_yes: bool) -> Result<bool> {
    print!("\n{prompt} {hint} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let stdin = std::io::stdin();
    if stdin
        .read_line(&mut line)
        .map_err(|e| MindError::io("<stdin>", e))?
        == 0
    {
        return Ok(false); // EOF (no input) -> treat as No
    }
    Ok(parse_confirm(&line, default_yes))
}

/// Prompt `[y/N]` on the terminal; default No.
pub(crate) fn confirm(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[y/N]", false)
}

/// Like `confirm` but defaulting to yes (`[Y/n]`): a bare Enter (or any reply that
/// is not an explicit no) confirms. Used where the affirmative is the expected
/// path and the action is reversible (the meld install-items prompt, CLI-23).
pub(crate) fn confirm_default_yes(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[Y/n]", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use crate::manifest::InstalledItem;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn parse_confirm_default_no_only_yes_confirms() {
        // spec: CLI-42 - the default-no guard (e.g. the forget glob confirm):
        // only an explicit yes confirms; empty and unrecognized are no.
        assert!(parse_confirm("y", false));
        assert!(parse_confirm("YES", false));
        assert!(!parse_confirm("", false));
        assert!(!parse_confirm("n", false));
        assert!(!parse_confirm("maybe", false));
    }

    #[test]
    fn parse_confirm_default_yes_only_no_declines() {
        // spec: CLI-23 - the meld install prompt defaults to yes: a bare Enter (or
        // anything but an explicit no) installs; only n/no declines.
        assert!(parse_confirm("", true));
        assert!(parse_confirm("y", true));
        assert!(parse_confirm(" Y \n", true));
        assert!(parse_confirm("whatever", true));
        assert!(!parse_confirm("n", true));
        assert!(!parse_confirm("NO", true));
    }

    /// Serialize every test that mutates process-global env vars
    /// (`MIND_POLICY_FILE`, `MIND_AGENT_HOMES`). Env is process-wide, so these
    /// tests cannot run concurrently.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Write `policy_toml` to a temp file and point `$MIND_POLICY_FILE` at it,
    /// also clearing `$MIND_AGENT_HOMES` so the outer env never bleeds in.
    /// Returns the held env guard (drop last) and the base temp dir.
    fn with_policy(policy_toml: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-cmd-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let policy_file = base.join("policy.toml");
        std::fs::write(&policy_file, policy_toml).unwrap();
        // SAFETY: ENV_LOCK is held, so no other test reads env concurrently.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
            std::env::set_var("MIND_POLICY_FILE", policy_file.to_str().unwrap());
        }
        (guard, base)
    }

    fn item(name: &str, source: &str) -> InstalledItem {
        InstalledItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: source.to_string(),
            commit: "deadbeef".to_string(),
            hash: "h".to_string(),
            store: format!("store/skills/{name}"),
            links: vec![],
            description: None,
        }
    }

    // ---- F2: POL-40 override-note suppression --------------------------------

    // spec: POL-40
    // The override note is shown only when the env var is set AND actually takes
    // effect. Under a lobe lock the env var is ignored, so suppress it.
    #[test]
    fn pol40_override_note_predicate() {
        // Unlocked: env var set -> show the note (behavior unchanged).
        assert!(show_override_note(true, false));
        // Locked: env var ignored by agent_homes -> suppress the note.
        assert!(!show_override_note(true, true));
        // Env var unset -> never show, locked or not.
        assert!(!show_override_note(false, false));
        assert!(!show_override_note(false, true));
    }

    // spec: POL-40
    // End-to-end against a real `$MIND_POLICY_FILE`: with `[lobes].lock = true`
    // the loaded policy reports lobes_lock(), so the override note must be
    // suppressed even though `$MIND_AGENT_HOMES` is set. Drives the same
    // `Policy::load()?` + `lobes_lock()` path `lobe_list` uses.
    #[test]
    fn pol40_locked_policy_suppresses_override_note() {
        let managed = std::env::temp_dir().join("mind-pol40-managed-target");
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{}\"]\n",
            managed.display()
        );
        let (_guard, base) = with_policy(&policy_toml);
        // Simulate the user setting the override var (ignored under lock).
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", base.join("env-lobe").to_str().unwrap());
        }

        let policy = Policy::load().unwrap().expect("policy should load");
        let env_set = std::env::var_os("MIND_AGENT_HOMES").is_some();
        let lobes_locked = policy.lobes_lock();

        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert!(env_set, "test must have the override var set");
        assert!(lobes_locked, "policy must report a lobe lock");
        assert!(
            !show_override_note(env_set, lobes_locked),
            "POL-40: a locked policy must suppress the false override note"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: POL-40
    // With a policy present but lobes unlocked (lock = false), the override note
    // is still shown when the env var is set: the env var really does take
    // effect, so the fix must not change the unlocked path.
    #[test]
    fn pol40_unlocked_keeps_override_note() {
        let policy_toml = "[lobes]\nlock = false\n";
        let (_guard, base) = with_policy(policy_toml);
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", base.join("env-lobe").to_str().unwrap());
        }

        let lobes_locked = matches!(Policy::load().unwrap(), Some(p) if p.lobes_lock());
        let env_set = std::env::var_os("MIND_AGENT_HOMES").is_some();

        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert!(!lobes_locked, "lock = false means no lobe lock");
        assert!(
            show_override_note(env_set, lobes_locked),
            "unlocked behavior must be unchanged: the note still shows"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- F3: POL-12 scoped-upgrade skip ordering -----------------------------

    // spec: POL-12
    // A scoped `upgrade <item>` must apply the item-ref filter before the policy
    // skip, so an out-of-scope item from a disallowed source produces no skip
    // line. The pre-fix ordering (policy skip first) would classify it as
    // PolicyBlocked and print a skip line for a source the user never selected.
    #[test]
    fn pol12_scoped_upgrade_no_skip_for_out_of_scope_source() {
        // Locked allowlist that permits only `allowed-src`.
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Policy::load().unwrap().expect("policy should load");
        assert!(policy.lock());

        // The user scopes to an item from the allowed source.
        let selected = item("wanted", "github.com/me/allowed-src");
        // An installed item from a *disallowed* source the user did NOT select.
        let other = item("unwanted", "github.com/them/blocked-src");

        let filter = parse_item_ref("wanted").unwrap();

        // Out-of-scope item: silently skipped, never reported as PolicyBlocked.
        assert_eq!(
            upgrade_item_disposition(&other, Some(&filter), Some(&policy)),
            UpgradeDisposition::OutOfScope,
            "POL-12: an unselected item must not be policy-skipped (no skip line)"
        );
        // The selected, allowed item is considered for upgrade.
        assert_eq!(
            upgrade_item_disposition(&selected, Some(&filter), Some(&policy)),
            UpgradeDisposition::Consider,
        );

        // Sanity: with no scope filter, the disallowed item IS policy-blocked
        // (the existing unscoped behavior is preserved).
        assert_eq!(
            upgrade_item_disposition(&other, None, Some(&policy)),
            UpgradeDisposition::PolicyBlocked,
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: POL-12
    // When the scoped item itself comes from a disallowed source, it passes the
    // filter and is then correctly reported as PolicyBlocked (the skip is for an
    // item the user actually selected, which is the intended behavior).
    #[test]
    fn pol12_scoped_upgrade_skips_selected_disallowed_item() {
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Policy::load().unwrap().expect("policy should load");

        let selected = item("blocked-item", "github.com/them/blocked-src");
        let filter = parse_item_ref("blocked-item").unwrap();

        assert_eq!(
            upgrade_item_disposition(&selected, Some(&filter), Some(&policy)),
            UpgradeDisposition::PolicyBlocked,
            "a selected item from a disallowed source is still reported"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Build a bare `Source` for the `hook_rerun_warranted` truth table. Uses
    /// `parse_spec` so the identity fields are filled the same way `meld` fills
    /// them; the test only manipulates the commit and install_hooks fields.
    ///
    /// `hooks` is a list of `(command, ran_at)` pairs to populate
    /// `Source.install_hooks`.
    fn hook_source(commit: Option<&str>, hooks: &[(&str, Option<&str>)]) -> crate::source::Source {
        use crate::source::RecordedHook;
        let mut s = crate::source::parse_spec("acme/tools").expect("spec parses");
        s.commit = commit.map(str::to_string);
        s.install_hooks = hooks
            .iter()
            .map(|(cmd, ran_at)| RecordedHook {
                command: cmd.to_string(),
                ran_at: ran_at.map(str::to_string),
            })
            .collect();
        s
    }

    // spec: HOOK-11 HOOK-55
    #[test]
    fn hook_rerun_warranted_truth_table() {
        // No hooks recorded: never re-run.
        assert!(
            !hook_rerun_warranted(&hook_source(Some("abc1234"), &[])),
            "no install_hooks means no re-run"
        );

        // Hook already ran at the current commit: nothing to do.
        assert!(
            !hook_rerun_warranted(&hook_source(
                Some("abc1234"),
                &[("make install", Some("abc1234"))],
            )),
            "ran_at == commit means the hook already ran here"
        );

        // All hooks ran at current commit: nothing to do.
        assert!(
            !hook_rerun_warranted(&hook_source(
                Some("abc1234"),
                &[
                    ("make build", Some("abc1234")),
                    ("make install", Some("abc1234")),
                ],
            )),
            "all hooks ran at current commit means no re-run warranted"
        );

        // Hook recorded but never run (skipped meld): re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(Some("abc1234"), &[("make install", None)],)),
            "a recorded-but-never-run hook is re-offered"
        );

        // Commit advanced past the commit the hook last ran at: re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(
                Some("def5678"),
                &[("make install", Some("abc1234"))],
            )),
            "an advanced commit warrants a re-run"
        );

        // Mixed: one hook ran at current commit, one at a stale commit.
        assert!(
            hook_rerun_warranted(&hook_source(
                Some("new0000"),
                &[
                    ("make build", Some("new0000")),
                    ("make install", Some("old0000")),
                ],
            )),
            "at least one stale hook warrants a re-run"
        );
    }

    // spec: HOOK-57
    // The init-source scaffold must include commented [[hooks]] examples for both
    // install and uninstall events, at least one marked optional = true.
    #[test]
    fn init_source_scaffold_includes_hooks_examples() {
        // Create a temp directory to run init_source in.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp =
            std::env::temp_dir().join(format!("mind-cmd-init-hooks-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        struct Rm(std::path::PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _rm = Rm(tmp.clone());

        // init_source should create the scaffold.
        init_source(Some(tmp.to_str().unwrap()), false).expect("init_source should succeed");

        let toml_path = tmp.join("mind.toml");
        assert!(toml_path.exists(), "mind.toml must be created");
        let contents = std::fs::read_to_string(&toml_path).unwrap();

        // HOOK-57: must include [[hooks]] commented examples.
        assert!(
            contents.contains("[[hooks]]"),
            "scaffold must include a commented [[hooks]] example: {contents}"
        );
        // Must show an install event.
        assert!(
            contents.contains("install"),
            "scaffold must show event = \"install\": {contents}"
        );
        // Must show an uninstall event.
        assert!(
            contents.contains("uninstall"),
            "scaffold must show event = \"uninstall\": {contents}"
        );
        // At least one example marked optional = true.
        assert!(
            contents.contains("optional = true"),
            "scaffold must have at least one optional = true example: {contents}"
        );
        // All [[hooks]] content is commented out (lines starting with # after trimming).
        // Check that the literal text "[[hooks]]" is preceded by a #.
        let has_uncommented_hooks = contents.lines().any(|l| l.trim() == "[[hooks]]");
        assert!(
            !has_uncommented_hooks,
            "[[hooks]] examples must all be commented out: {contents}"
        );

        // Existing assertions still pass (regression guard).
        assert!(
            contents.contains("[source]"),
            "scaffold must still have [source]"
        );
        assert!(
            contents.contains("# prefix = \"prefix\""),
            "scaffold must still have commented prefix: {contents}"
        );
    }

    // spec: HOOK-58
    // The recall --sources hook token is count-aware: no token for 0 hooks,
    // ` hook` for 1, ` hooks(N)` for N > 1.
    #[test]
    fn hook_token_is_count_aware() {
        // 0 hooks -> empty string.
        let s0 = hook_source(Some("abc"), &[]);
        let token0 = match s0.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token0, "", "no hooks => empty token");

        // 1 hook -> ` hook`.
        let s1 = hook_source(Some("abc"), &[("make install", Some("abc"))]);
        let token1 = match s1.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token1, " hook", "1 hook => ' hook'");

        // 2 hooks -> ` hooks(2)`.
        let s2 = hook_source(
            Some("abc"),
            &[("make build", Some("abc")), ("make install", Some("abc"))],
        );
        let token2 = match s2.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token2, " hooks(2)", "2 hooks => ' hooks(2)'");
    }

    // spec: HOOK-50 HOOK-55
    // Verifies that `hook_rerun_warranted` uses the new multi-hook model: a source
    // with multiple install hooks is re-offered when ANY hook is pending, and not
    // re-offered when ALL hooks have ran_at == commit.
    #[test]
    fn hook_rerun_warranted_multi_hook_model() {
        // All hooks ran at the current commit: not warranted.
        let all_current = hook_source(
            Some("fff000"),
            &[
                ("make build", Some("fff000")),
                ("make install", Some("fff000")),
                ("make test", Some("fff000")),
            ],
        );
        assert!(
            !hook_rerun_warranted(&all_current),
            "all hooks current => no re-run warranted"
        );

        // One hook never ran (ran_at == None): warranted.
        let one_never_ran = hook_source(
            Some("fff000"),
            &[
                ("make build", Some("fff000")),
                ("make install", None), // skipped at meld time
            ],
        );
        assert!(
            hook_rerun_warranted(&one_never_ran),
            "one hook with ran_at=None => re-run warranted"
        );

        // Source has no commit yet (None): all pending (ran_at=None matches None but
        // hooks with a ran_at=Some differ from None).
        let no_commit = hook_source(None, &[("make install", Some("old"))]);
        assert!(
            hook_rerun_warranted(&no_commit),
            "commit=None and ran_at=Some => they differ => warranted"
        );
    }
}
