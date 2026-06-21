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

/// Whether `upgrade` should re-offer a source's install hook (HOOK-11): a hook is
/// in effect AND the commit it last ran at differs from the source's current
/// commit. A None `install_hook_commit` (a recorded-but-never-run hook from a
/// skipped meld) differs from any Some(commit), so it is re-offered too.
fn hook_rerun_warranted(source: &crate::source::Source) -> bool {
    source.install_hook.is_some()
        && source.install_hook_commit.as_deref() != source.commit.as_deref()
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

    let dir = source.clone_dir(paths);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
    }
    if let Some(parent) = dir.parent() {
        crate::paths::mkdir_p(parent)?;
    }

    // We need to clone before reading mind.toml (it lives inside the clone).
    // For the initial clone we use the default branch so we can read the file,
    // then immediately re-clone at the resolved pin if needed. For local/file://
    // repos this is cheap; for real remotes the first clone is always needed.
    //
    // Optimisation: if the consumer already specified a pin (consumer_pin is
    // Some), or if after reading the mindfile we get a directive, we re-clone at
    // the right point. For DefaultBranch the first clone is already correct.

    println!("melding {} from {}", source.name, source.url);

    // Step 1: clone the default branch to read mind.toml.
    git::clone(&source.url, &dir)?;
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
            let _ = std::fs::remove_dir_all(&dir);
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
            let _ = std::fs::remove_dir_all(&dir);
            return Err(MindError::UnpinnedSourceForbidden { identity });
        }
    }

    // Step 3: if the effective pin is not DefaultBranch, re-clone at that point.
    // A pin that does not resolve in the remote is a `Git` error and must leave
    // nothing behind (CLI-18). `clone_at` for a ref clones first and then checks
    // out the sha, so a bad ref leaves a half-built clone dir; clean it up on any
    // failure of the re-clone so no orphan is left on disk.
    if effective_pin != Pin::DefaultBranch {
        std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
        if let Err(e) = git::clone_at(&source.url, &dir, &effective_pin) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        // The pin may land on a different mind.toml than the default branch.
        // Reload it so all downstream reads (description, the DSC-52
        // is_authoritative gate, and the nested [discover].sources loop) see the
        // pinned content, not the default branch's. The catalog scan below
        // already re-reads mind.toml from disk, so item discovery was correct;
        // only these in-memory reads were stale.
        mindfile = MindToml::load(&dir)?;
        source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());
    }

    source.pin = effective_pin;
    source.commit = Some(git::head_commit(&source.url, &dir)?);

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

    // CLI-24: a source that declares `[source].prefix` and was not melded with an
    // explicit `--as` prefix prompts (interactively) whether to namespace its
    // items under that prefix. The choice becomes the source alias, so the scan
    // and the later install use the chosen names. Non-interactive runs accept the
    // declared prefix as-is (alias stays None).
    if top_level
        && source.alias.is_none()
        && crate::hook::is_tty()
        && let Some(declared) = mindfile.as_ref().and_then(|m| m.source.prefix.clone())
        && !declared.is_empty()
    {
        let answer = prompt_line(&format!(
            "{} suggests the prefix '{declared}'.\n  use it? [Y]es / type a different prefix / [n]o prefix: ",
            source.name
        ))?;
        source.alias = crate::namespace::prefix_choice(&answer);
    }

    // Scan before registering. If the source is rejected here (e.g. the
    // version gate, DSC-40), remove the clone so no orphan is left on disk.
    let items = match catalog::scan(paths, &single(&source)) {
        Ok(items) => items,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    warn_unguarded_references(&items);
    println!("melded {} ({} item(s))", source.name, items.len());

    // Install hook (HOOK-10): the working tree is now checked out at the resolved
    // pin, so the hook runs in the right tree. Resolve the effective hook (a
    // consumer `--install-hook` overrides a declared `[source].install`).
    {
        let declared = mindfile.as_ref().and_then(|m| m.source.install.clone());
        let supplied = install_hook;
        match crate::hook::resolve_hook(declared.as_deref(), supplied.as_deref()) {
            None => {
                // HOOK-3: no hook in effect; proceed unchanged.
            }
            Some((cmd, overrides)) => {
                let cmd = cmd.to_string();
                let pin_desc = pin_description(&source.pin);
                let commit = source.commit.clone().unwrap_or_default();
                let clone_path = dir.display().to_string();

                // Decide run / skip / abort.
                let choice = if dangerously_skip_hook_check {
                    // HOOK-23: run without prompting.
                    println!(
                        "note: running install hook for {} without the safety prompt (--dangerously-skip-install-hook-check)",
                        source.name
                    );
                    crate::hook::HookChoice::RunAndContinue
                } else if !crate::hook::is_tty() {
                    // HOOK-22: no TTY; never run silently, never abort. Take the
                    // skip path.
                    crate::hook::HookChoice::SkipAndContinue
                } else {
                    let disclosure = crate::hook::disclosure_text(
                        &source.name,
                        &pin_desc,
                        &commit,
                        &clone_path,
                        &cmd,
                        if overrides { declared.as_deref() } else { None },
                    );
                    crate::hook::prompt_choice(&disclosure)?
                };

                match choice {
                    crate::hook::HookChoice::RunAndContinue => {
                        // HOOK-30: a non-zero exit fails the meld; remove the clone
                        // so the source is not left registered.
                        if let Err(e) = crate::hook::run_hook(&cmd, &dir, &source.name) {
                            let _ = std::fs::remove_dir_all(&dir);
                            return Err(e);
                        }
                        // HOOK-31: record the command and the commit it ran at.
                        source.install_hook = Some(cmd);
                        source.install_hook_commit = source.commit.clone();
                    }
                    crate::hook::HookChoice::SkipAndContinue => {
                        // HOOK-21 / HOOK-22: install the source and its items, but
                        // do not build the tooling. Record the hook command so
                        // `upgrade` can re-offer it, but leave install_hook_commit
                        // None (the hook has not been run).
                        println!(
                            "note: skipped the install hook for {}; its items may not work until the hook is run",
                            source.name
                        );
                        source.install_hook = Some(cmd);
                    }
                    crate::hook::HookChoice::Abort => {
                        // HOOK-21: aborting installs nothing; the source is not
                        // registered.
                        let _ = std::fs::remove_dir_all(&dir);
                        println!("aborted; nothing installed");
                        return Ok(0);
                    }
                }
            }
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
    // and the ones it mentions in bare prose (templating candidates).
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    let mut any_bare = false;
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
        if tokens.is_empty() && bare.is_empty() {
            continue;
        }
        println!("  {} {} references:", it.kind, it.name);
        if !tokens.is_empty() {
            println!("    tokenized: {}", tokens.join(", "));
        }
        if !bare.is_empty() {
            any_bare = true;
            println!("    bare (templating candidates): {}", bare.join(", "));
        }
    }
    if any_bare && !template {
        println!(
            "  run `mind init-source {dir} --template` to wrap the bare references as {{{{ns:name}}}}"
        );
    }

    // mind.toml scaffold (INIT-3): create only when absent; never overwrite.
    let toml_path = root.join("mind.toml");
    if toml_path.exists() {
        println!("  mind.toml already exists; left unchanged");
    } else {
        let scaffold = "[source]\ndescription = \"\"   # what this source offers\n# prefix = \"jk\"    # namespace items as jk-<name>\n";
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

/// `mind unmeld <name> [--forget]` — drop a source. `name` may be the full
/// `owner/repo` or an unambiguous repo basename. With `--forget`, every item
/// installed from the source is uninstalled (via its file registry) first.
pub fn unmeld(paths: &Paths, name: &str, forget: bool) -> Result<()> {
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

    let source = registry.sources.remove(idx);

    // With --forget, remove every item installed from this source before the
    // source itself, each via its recorded file registry, then re-key the manifest.
    let mut forgotten = 0;
    if forget {
        let mut manifest = Manifest::load(paths)?;
        let keys: Vec<String> = manifest
            .items
            .values()
            .filter(|it| it.source == source.name)
            .map(|it| it.key())
            .collect();
        for key in keys {
            if let Some(item) = manifest.items.remove(&key) {
                install::uninstall(paths, &item)?;
                forgotten += 1;
            }
        }
        manifest.save(paths)?;
    }

    let dir = source.clone_dir(paths);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
    }
    registry.save(paths)?;
    if forget {
        println!(
            "unmelded {} ({forgotten} installed item(s) removed)",
            source.name
        );
    } else {
        println!(
            "unmelded {} (installed items left untouched; `mind forget` to remove them)",
            source.name
        );
    }
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
pub fn learn(paths: &Paths, item_ref: &str, dry_run: bool, yes: bool) -> Result<()> {
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
        println!("(dry run) would learn {} item(s):", closure.len());
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
        match install::install(paths, target, &commit, &siblings) {
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
pub fn install_melded_source(paths: &Paths, repo: &str, yes: bool) -> Result<()> {
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
        return learn(paths, &item_ref, false, true);
    }
    if !crate::hook::is_tty() {
        println!(
            "note: {source_name} has {} item(s) to install; run `mind learn '{item_ref}'` (or re-meld with --yes)",
            plan.install_count
        );
        return Ok(());
    }

    // Interactive: show the install preview (the dry-run list), then prompt.
    learn(paths, &item_ref, true, false)?;
    if confirm(&format!(
        "install these {} item(s) now?",
        plan.install_count
    ))? {
        learn(paths, &item_ref, false, true)
    } else {
        println!("skipped; run `mind learn '{item_ref}'` to install later");
        Ok(())
    }
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
pub fn forget(paths: &Paths, item_ref: &str) -> Result<()> {
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
        // succeeds (transactional install).
        let installed = install::install(paths, &up.cat, &up.new_commit, &siblings)?;
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

/// HOOK-11: re-run each source's install hook when warranted (the source
/// advanced past the commit the hook last ran at, or the hook was recorded but
/// never run). Same trust boundary as `meld`: prompt and disclose, unless the
/// `--dangerously-skip-install-hook-check` flag is set or there is no TTY. In
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
        // `hook_rerun_warranted` guarantees install_hook is Some.
        let cmd = source
            .install_hook
            .clone()
            .expect("warranted re-run has a hook command");
        let dir = source.clone_dir(paths);
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let clone_path = dir.display().to_string();

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
            // Abort is treated as Skip here (the source is already registered).
            matches!(
                crate::hook::prompt_choice(&disclosure)?,
                crate::hook::HookChoice::RunAndContinue
            )
        };

        if run {
            // HOOK-30: a non-zero exit is a hard error. The source stays
            // registered (it is an existing install), so do not remove anything;
            // just propagate the failure.
            crate::hook::run_hook(&cmd, &dir, &source.name)?;
            // HOOK-31: record the commit the hook ran at.
            source.install_hook_commit = source.commit.clone();
            changed = true;
            println!("re-ran install hook for {}", source.name);
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
                // HOOK-31: surface that a source carries an install hook.
                let hook = if s.install_hook.is_some() {
                    " hook"
                } else {
                    ""
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

    let filtered: Vec<&crate::manifest::InstalledItem> = manifest
        .items
        .values()
        .filter(|it| {
            kind.is_none_or(|k| it.kind == k)
                && source.is_none_or(|s| source_matches(&it.source, s))
        })
        .collect();
    if json {
        return print_json(&filtered);
    }
    if manifest.items.is_empty() {
        println!("nothing learned yet; `mind probe` to see what's available");
        return Ok(());
    }
    if filtered.is_empty() {
        println!("no installed items match the filter");
        return Ok(());
    }
    let rows = filtered
        .iter()
        .map(|it| {
            vec![
                it.key(),
                it.source.clone(),
                short(&it.commit),
                summary(it.description.as_deref(), 60),
            ]
        })
        .collect::<Vec<_>>();
    print_rows(&rows);
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

    // Print hard findings first, then advisory.
    for f in &result.hard {
        eprintln!("error [{}]: {}", f.kind, f.message);
    }
    for f in &result.advisory {
        println!("advisory [{}]: {}", f.kind, f.message);
    }

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

/// Prompt `[y/N]` on the terminal; default No.
pub(crate) fn confirm(prompt: &str) -> Result<bool> {
    print!("\n{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let stdin = std::io::stdin();
    if stdin
        .read_line(&mut line)
        .map_err(|e| MindError::io("<stdin>", e))?
        == 0
    {
        return Ok(false); // EOF -> treat as No
    }
    let ans = line.trim().to_ascii_lowercase();
    Ok(ans == "y" || ans == "yes")
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
    /// them; the test only manipulates the commit and hook fields.
    fn hook_source(
        commit: Option<&str>,
        install_hook: Option<&str>,
        install_hook_commit: Option<&str>,
    ) -> crate::source::Source {
        let mut s = crate::source::parse_spec("acme/tools").expect("spec parses");
        s.commit = commit.map(str::to_string);
        s.install_hook = install_hook.map(str::to_string);
        s.install_hook_commit = install_hook_commit.map(str::to_string);
        s
    }

    // spec: HOOK-11
    #[test]
    fn hook_rerun_warranted_truth_table() {
        // No hook in effect: never re-run, whatever the commits.
        assert!(
            !hook_rerun_warranted(&hook_source(Some("abc1234"), None, None)),
            "no install_hook means no re-run"
        );
        assert!(
            !hook_rerun_warranted(&hook_source(Some("abc1234"), None, Some("old0000"))),
            "no install_hook means no re-run even with a recorded run commit"
        );

        // Hook already ran at the current commit: nothing to do.
        assert!(
            !hook_rerun_warranted(&hook_source(
                Some("abc1234"),
                Some("make install"),
                Some("abc1234"),
            )),
            "install_hook_commit == commit means the hook already ran here"
        );

        // Hook recorded but never run (skipped meld): re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(Some("abc1234"), Some("make install"), None)),
            "a recorded-but-never-run hook is re-offered"
        );

        // Commit advanced past the commit the hook last ran at: re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(
                Some("def5678"),
                Some("make install"),
                Some("abc1234"),
            )),
            "an advanced commit warrants a re-run"
        );
    }
}
