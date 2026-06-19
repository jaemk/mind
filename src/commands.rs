//! Command implementations, one public function per CLI verb.

use std::collections::HashSet;
use std::io::Write;

use crate::catalog::{self, CatalogItem};
use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::git;
use crate::hash::hash_path;
use crate::install;
use crate::manifest::Manifest;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::resolve::{is_glob, parse_item_ref, resolve, select, select_installed, source_matches};
use crate::source::{Registry, parse_spec};

/// `mind meld <repo> [--as <prefix>]` — register and clone a source.
///
/// If the source's `mind.toml` lists nested `[discover].sources`, each is melded
/// too (recursively), so a repo can act as a curated super-source. Nested
/// sources are skipped if already registered, and cycles are guarded by URL.
pub fn meld(paths: &Paths, repo: &str, alias: Option<String>) -> Result<()> {
    paths.ensure_layout()?;
    let mut registry = Registry::load(paths)?;
    let mut visited = HashSet::new();
    let added = meld_recursive(paths, &mut registry, repo, alias, true, &mut visited)?;
    registry.save(paths)?;
    if added > 1 {
        println!("melded {added} source(s)");
    }
    Ok(())
}

/// Meld one source and then its nested sources. Returns how many sources were
/// newly added to the registry. `top_level` distinguishes the user's own meld
/// (errors on a duplicate) from a curated nested meld (skips a duplicate).
fn meld_recursive(
    paths: &Paths,
    registry: &mut Registry,
    repo: &str,
    alias: Option<String>,
    top_level: bool,
    visited: &mut HashSet<String>,
) -> Result<usize> {
    let mut source = parse_spec(repo)?;
    source.alias = alias;

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

    println!("melding {} from {}", source.name, source.url);
    git::clone(&source.url, &dir)?;
    source.commit = Some(git::head_commit(&source.url, &dir)?);
    let mindfile = MindToml::load(&dir)?;
    source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());

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
    registry.sources.push(source);

    let mut added = 1;
    if let Some(nested) = mindfile
        .as_ref()
        .and_then(|m| m.discover.as_ref())
        .map(|d| &d.sources)
    {
        for entry in nested {
            added += meld_recursive(
                paths,
                registry,
                &entry.source,
                entry.alias.clone(),
                false,
                visited,
            )?;
        }
    }
    Ok(added)
}

/// Warn when a namespaced source references siblings in bare prose, which
/// prefixing will break unless rewritten as `{{ns:name}}` tokens.
fn warn_unguarded_references(items: &[CatalogItem]) {
    // Only meaningful once a prefix is in effect.
    if !items.iter().any(|it| it.prefix.is_some()) {
        return;
    }
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    for item in items {
        let Ok(content) = std::fs::read_to_string(meta_path(item)) else {
            continue;
        };
        let mut refs = crate::namespace::unguarded_refs(&content, &siblings);
        refs.retain(|r| r != &item.name); // self-mentions are fine
        if !refs.is_empty() {
            eprintln!(
                "warning: {} references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                item.key(),
                refs.join(", ")
            );
        }
    }
}

/// The file whose text describes/uses an item (SKILL.md for skills).
fn meta_path(item: &CatalogItem) -> std::path::PathBuf {
    match item.kind {
        crate::error::ItemKind::Skill => item.path.join("SKILL.md"),
        _ => item.path.clone(),
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

/// `mind learn <item> [--dry-run]` — install one item, or many via a glob.
pub fn learn(paths: &Paths, item_ref: &str, dry_run: bool) -> Result<()> {
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

    // Two matches that install under the same name would clobber each other.
    if let Some((key, sources)) = colliding_install(&targets) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    if dry_run {
        println!("(dry run) would learn {} item(s):", targets.len());
        let rows = targets
            .iter()
            .map(|t| vec![t.key(), t.source.clone()])
            .collect::<Vec<_>>();
        print_rows(&rows);
        return Ok(());
    }

    // Install each target. If one fails mid-batch, stop but still persist the
    // items already installed, so the manifest always matches what is on disk.
    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    for target in &targets {
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

/// `mind sync [--evolve]` — fetch every source and refresh its recorded commit.
/// With `--evolve`, an `evolve` pass runs after the refresh (reporting pending
/// upgrades and prompting before applying, exactly like `mind evolve`), so one
/// command both fetches upstream and applies pending upgrades.
pub fn sync(paths: &Paths, then_evolve: bool) -> Result<()> {
    let mut registry = Registry::load(paths)?;
    if registry.sources.is_empty() {
        println!("no sources melded; run `mind meld <owner/repo>`");
        return Ok(());
    }
    for source in &mut registry.sources {
        let dir = source.clone_dir(paths);
        print!("syncing {} ... ", source.name);
        let _ = std::io::stdout().flush();
        if dir.join(".git").is_dir() {
            git::fetch_and_reset(&source.url, &dir)?;
        } else {
            if let Some(parent) = dir.parent() {
                crate::paths::mkdir_p(parent)?;
            }
            git::clone(&source.url, &dir)?;
        }
        let new_commit = git::head_commit(&source.url, &dir)?;
        let changed = source.commit.as_deref() != Some(new_commit.as_str());
        source.commit = Some(new_commit.clone());
        source.description = MindToml::load(&dir)?.and_then(|mt| mt.source.description);
        println!(
            "{} ({})",
            if changed { "updated" } else { "up to date" },
            short(&new_commit)
        );
    }
    registry.save(paths)?;
    if then_evolve {
        evolve(paths, false, None)?;
    }
    Ok(())
}

/// `mind evolve [--yes] [item]` — report and optionally apply upgrades.
pub fn evolve(paths: &Paths, yes: bool, item_ref: Option<&str>) -> Result<()> {
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;

    let filter = item_ref.map(parse_item_ref).transpose()?;
    let mut pending: Vec<Upgrade> = Vec::new();

    for installed in manifest.items.values() {
        if let Some(f) = &filter {
            // Limit to the matching installed item(s): the effective name, plus
            // the kind prefix and source qualifier when the ref gives them. A ref
            // may legitimately match several installed items, all of which evolve.
            if !crate::resolve::installed_matches(installed, f) {
                continue;
            }
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
            println!("evolved {} -> {}", up.old.key(), installed.key());
        } else {
            println!("evolved {}", installed.key());
        }
        manifest.insert(installed);
    }
    manifest.save(paths)?;
    Ok(())
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

/// `mind recall [--sources] [item] [--kind K] [--source S]`. The `--kind` and
/// `--source` filters narrow the installed-items listing; they do not apply to
/// `--sources` or to a single-item lookup (use a `kind:`/`owner/repo#` ref there).
pub fn recall(
    paths: &Paths,
    sources: bool,
    item: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
) -> Result<()> {
    if sources {
        let registry = Registry::load(paths)?;
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
                vec![
                    s.name.clone(),
                    s.url.clone(),
                    format!("[{commit}{ns}]"),
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

    if manifest.items.is_empty() {
        println!("nothing learned yet; `mind probe` to see what's available");
        return Ok(());
    }
    let filtered: Vec<_> = manifest
        .items
        .values()
        .filter(|it| {
            kind.is_none_or(|k| it.kind == k)
                && source.is_none_or(|s| source_matches(&it.source, s))
        })
        .collect();
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

/// `mind probe [query] [--kind K] [--source S]`. A leading `*` marks installed
/// items; the hash is of the current source content. `--kind` and `--source`
/// narrow the listing and compose with the substring query.
pub fn probe(
    paths: &Paths,
    query: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let q = query.unwrap_or("");
    let mut hits: Vec<&CatalogItem> = items
        .iter()
        .filter(|it| {
            (q.is_empty() || it.effective_name().contains(q))
                && kind.is_none_or(|k| it.kind == k)
                && source.is_none_or(|s| source_matches(&it.source, s))
        })
        .collect();
    hits.sort_by_key(|a| a.key());

    if hits.is_empty() {
        if registry.sources.is_empty() {
            println!("no sources melded; run `mind meld <owner/repo>`");
        } else {
            println!("no items match '{q}'");
        }
        return Ok(());
    }

    let installed = |it: &CatalogItem| {
        manifest
            .items
            .values()
            .any(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name)
    };
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

/// `mind introspect [--fix]` — report drift and breakage. With `--fix`, repair
/// what can be fixed without changing versions: recreate missing symlinks from
/// each item's file registry. Drifted or renamed items are left to `evolve`.
pub fn introspect(paths: &Paths, fix: bool) -> Result<()> {
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let mut issues = 0;

    for s in &registry.sources {
        if !s.clone_dir(paths).join(".git").is_dir() {
            println!("source '{}' has no clone on disk; run `mind sync`", s.name);
            issues += 1;
        } else if s.commit.is_none() {
            println!("source '{}' was never synced; run `mind sync`", s.name);
            issues += 1;
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
            let repaired = if fix { install::relink(paths, it)? } else { 0 };
            if repaired > 0 {
                println!("{}: relinked {repaired} missing symlink(s)", it.key());
            }
            for link in &missing {
                if std::fs::symlink_metadata(link).is_err() {
                    println!("{}: symlink missing at {link}", it.key());
                    issues += 1;
                }
            }
        }
        // Match on stable identity (source, kind, bare_name).
        match catalog
            .iter()
            .find(|c| c.kind == it.kind && c.name == it.bare_name && c.source == it.source)
        {
            None => {
                println!("{}: no longer present in source '{}'", it.key(), it.source);
                issues += 1;
            }
            Some(cat) => {
                if cat.effective_name() != it.name {
                    println!(
                        "{}: namespace changed to '{}'; run `mind evolve`",
                        it.key(),
                        cat.effective_name()
                    );
                    issues += 1;
                } else if let Ok(h) = hash_path(&cat.path)
                    && h != it.hash
                {
                    println!("{}: upstream changed; run `mind evolve`", it.key());
                    issues += 1;
                }
            }
        }
    }

    if issues == 0 {
        println!(
            "all good: {} source(s), {} item(s) installed",
            registry.sources.len(),
            manifest.items.len()
        );
    } else {
        println!("\n{issues} issue(s) found");
    }
    Ok(())
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
    if std::env::var_os("MIND_AGENT_HOMES").is_some() {
        println!("note: MIND_AGENT_HOMES is set and overrides the above");
    }
    Ok(())
}

/// `mind config lobes remove <path>` — drop an agent home.
pub fn lobe_remove(paths: &Paths, path: &str) -> Result<()> {
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

// --- helpers ---------------------------------------------------------------

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

/// Prompt `[y/N]` on the terminal; default No.
fn confirm(prompt: &str) -> Result<bool> {
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
