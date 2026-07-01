//! Scanning melded sources for installable items.
//!
//! By convention (mirrors the `agents` repo layout):
//! - `skills/<name>/SKILL.md`  -> skill `<name>`
//! - `agents/<name>.md`        -> agent `<name>`
//! - `rules/<name>.md`         -> rule  `<name>`
//!
//! A source may instead ship a `mind.toml` (see [`crate::mindfile`]) declaring
//! its inventory explicitly via `[[items]]` or `[discover]` globs; that takes
//! over discovery for the source. Either way, an item's `description` is read
//! from its frontmatter unless overridden.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{ItemKind, MindError, Result};
use crate::frontmatter;
use crate::mindfile::{Discover, HookEvent, ItemDecl, KindGlobs, MindToml, ResolvedHook};
use crate::namespace;
use crate::paths::Paths;
use crate::source::{Registry, Source};

/// One installable item discovered in a source.
///
/// The catalog is source truth: `name` is the item's *bare* name exactly as it
/// appears in the repo. The namespace prefix and `{{ns:}}` token expansion are
/// install-time transforms, applied by `install.rs`, not baked in here. The
/// stable identity of an item is therefore `(source, kind, name)`, which is what
/// `upgrade` matches on across a prefix change.
#[derive(Debug, Clone)]
pub struct CatalogItem {
    pub kind: ItemKind,
    /// Bare name as it appears in the source.
    pub name: String,
    /// The source `name` it belongs to.
    pub source: String,
    /// The source's effective namespace prefix, if any (applied at install).
    pub prefix: Option<String>,
    /// Path to the item root on disk (a dir for skills, a file for agents/rules).
    pub path: PathBuf,
    /// One-line description, from frontmatter or a `mind.toml` override.
    pub description: Option<String>,
    /// Optional link target relative to `~/.claude` (from `mind.toml`); `None`
    /// means use the default location for the kind.
    pub link_rel: Option<String>,
    /// A tool's entrypoint, relative to the item dir (from `TOOL.md` frontmatter
    /// or a `mind.toml` override). What `{{tools:name}}` resolves to. Tools only.
    pub bin: Option<String>,
    /// A per-item build command run in staging at install (from `TOOL.md`
    /// frontmatter or a `mind.toml` override). `None` means no build step.
    pub build: Option<String>,
    /// An item install hook (HOOK-80): a host side-effect command run as the
    /// final install step (from a `mind.toml` `[[items]].install` on any kind,
    /// or a tool's `TOOL.md` `install:` frontmatter). `None` means none.
    pub install: Option<String>,
    /// An item uninstall hook (HOOK-80): a host cleanup command run when the item
    /// is removed (from a `mind.toml` `[[items]].uninstall` on any kind, or a
    /// tool's `TOOL.md` `uninstall:` frontmatter). `None` means none.
    pub uninstall: Option<String>,
    /// Explicit intra-source dependency refs declared in the item's frontmatter
    /// `requires:` key (DEP-4). Whitespace-split raw strings as written, e.g.
    /// `["skill:x", "agent:y"]`. Empty when absent.
    pub requires: Vec<String>,
    /// The item's full resolved lifecycle hooks (HOOK-86), in execution order:
    /// the scalar `install`/`uninstall` shorthand folded in ahead of any
    /// `[[items.hooks]]` array entries. The scalar fields above stay populated
    /// alongside this list (HOOK-85 disclosure reads them); this list is what the
    /// install/uninstall execution iterates. A `TOOL.md`-frontmatter item has
    /// only its scalars folded in (DSC-21: the array form requires `mind.toml`).
    pub hooks: Vec<ResolvedHook>,
}

impl CatalogItem {
    /// The name this item installs under: bare, or `<prefix>:<bare>` if namespaced.
    pub fn effective_name(&self) -> String {
        namespace::apply(&self.name, &self.prefix)
    }

    /// The harness-visible name for an agent: the frontmatter `name:` field when
    /// it is non-empty and a safe single path component, else the bare catalog
    /// name (`self.name`). Returns `None` for non-agent kinds.
    ///
    /// The Claude harness keys an agent by its frontmatter `name`, not its
    /// filename, so this is the name mind links the agent under in each agent
    /// home (NS-40).
    pub fn agent_harness_name(&self) -> Option<String> {
        // spec: NS-40
        if self.kind != ItemKind::Agent {
            return None;
        }
        if let Some(fm_name) = frontmatter::file_field(&self.path, "name") {
            let trimmed = fm_name.trim().to_string();
            if !trimmed.is_empty() && is_safe_item_name(&trimmed) {
                return Some(trimmed);
            }
        }
        // Fall back to the bare catalog name (file stem).
        Some(self.name.clone())
    }

    /// A tool's entrypoint relative to its dir, for `{{tools:name}}`: the
    /// declared `bin`, else the convention default `<name>` (a file named after
    /// the tool at the dir root) when that file is present in the source. `None`
    /// for non-tools or a tool with no resolvable entrypoint.
    pub fn resolved_bin(&self) -> Option<String> {
        if self.kind != ItemKind::Tool {
            return None;
        }
        if let Some(bin) = &self.bin {
            return Some(bin.clone());
        }
        self.path
            .join(&self.name)
            .is_file()
            .then(|| self.name.clone())
    }

    /// This item's resolved install hooks (HOOK-86), in execution order.
    pub fn install_hooks(&self) -> Vec<&ResolvedHook> {
        self.hooks
            .iter()
            .filter(|h| h.event == HookEvent::Install)
            .collect()
    }

    /// This item's resolved uninstall hooks (HOOK-86), in execution order.
    pub fn uninstall_hooks(&self) -> Vec<&ResolvedHook> {
        self.hooks
            .iter()
            .filter(|h| h.event == HookEvent::Uninstall)
            .collect()
    }

    /// User-facing key, using the effective (possibly prefixed) name.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.effective_name())
    }

    /// This item as a path-token resolution sibling (namespace.rs), carrying its
    /// kind, bare name, and resolved `bin`. `PathSibling` exists so `namespace`
    /// need not depend on `catalog`; this is the one place the mapping lives.
    pub fn as_path_sibling(&self) -> namespace::PathSibling {
        namespace::PathSibling {
            kind: self.kind,
            name: self.name.clone(),
            bin: self.resolved_bin(),
        }
    }
}

/// True when `query` matches the item by effective name or description,
/// case-insensitively. An empty query matches everything. (spec: CLI-85)
pub(crate) fn matches_query(item: &CatalogItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if item.effective_name().to_lowercase().contains(&q) {
        return true;
    }
    item.description
        .as_deref()
        .is_some_and(|d| d.to_lowercase().contains(&q))
}

/// Scan every melded source for installable items.
pub fn scan(paths: &Paths, registry: &Registry) -> Result<Vec<CatalogItem>> {
    let mut items = Vec::new();
    for source in &registry.sources {
        scan_source(paths, source, &mut items)?;
    }
    Ok(items)
}

pub(crate) fn scan_source(
    paths: &Paths,
    source: &Source,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    let clone_root = source.clone_dir(paths);
    scan_source_at(clone_root, source, out)
}

/// Scan a source whose clone root is known directly (e.g. for `review`, where
/// the directory may not live under the standard sources tree).
pub(crate) fn scan_source_at(
    clone_root: impl AsRef<std::path::Path>,
    source: &Source,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    let clone_root = clone_root.as_ref();
    let mindfile = MindToml::load(clone_root)?;

    // Reject a source that requires a newer `mind` than the one running, rather
    // than scanning it against a format this version may predate (DSC-40).
    if let Some(required) = mindfile
        .as_ref()
        .and_then(|m| m.source.min_mind_version.as_deref())
        && !crate::mindfile::version_at_least(env!("CARGO_PKG_VERSION"), required)
    {
        return Err(MindError::IncompatibleVersion {
            source_name: source.name.clone(),
            required: required.to_string(),
            running: env!("CARGO_PKG_VERSION").to_string(),
        });
    }

    // Effective prefix: consumer alias wins over the repo's own declaration. An
    // empty alias (`--as ''`, or the meld prompt's "no prefix" choice) is the
    // explicit "no prefix" override and suppresses a declared `[source].prefix`.
    // No NS-25 guard is needed here: both inputs are validated upstream where they
    // are set (the `--as` alias in commands.rs, the `[source].prefix` at mindfile
    // load), so a reserved-kind-word prefix can never reach this resolution.
    let prefix = source
        .alias
        .clone()
        .or_else(|| mindfile.as_ref().and_then(|m| m.source.prefix.clone()))
        .filter(|p| !p.is_empty());

    match mindfile {
        Some(mt) if mt.is_authoritative() => {
            // spec: DSC-52 — authoritative mind.toml ignores scan roots entirely;
            // its paths are always repo-root-relative.
            // spec: DSC-53 — (kind, bare_name) uniqueness applies to [[items]]
            // declarations: two entries with the same kind+name are a DuplicateItem.
            let mut seen: std::collections::HashSet<(crate::error::ItemKind, String)> =
                std::collections::HashSet::new();
            for decl in &mt.items {
                let item = from_decl(clone_root, source, &prefix, decl)?;
                let key = (item.kind, item.name.clone());
                if !seen.insert(key.clone()) {
                    return Err(MindError::DuplicateItem {
                        source_name: source.name.clone(),
                        kind: key.0,
                        name: key.1,
                    });
                }
                out.push(item);
            }
            if let Some(discover) = &mt.discover {
                scan_globs(clone_root, source, &prefix, discover, out)?;
            }
            Ok(())
        }
        ref mt => {
            // spec: DSC-50 / DSC-51 — resolve the effective scan roots:
            //   source.roots (--root override) wins; else mindfile [source].roots;
            //   else implicit single root of the repo root.
            let effective_roots: Vec<String> = source
                .roots
                .clone()
                .or_else(|| mt.as_ref().and_then(|m| m.source.roots.clone()))
                .unwrap_or_else(|| vec![".".to_string()]);

            // Validate each root: must exist as a directory inside the clone and
            // must not be absolute or escape the clone via `..`.
            for r in &effective_roots {
                if std::path::Path::new(r).is_absolute() {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
                let full = clone_root.join(r);
                // Reject paths that try to escape via `..`.
                if !full
                    .canonicalize()
                    .unwrap_or_else(|_| full.clone())
                    .starts_with(
                        clone_root
                            .canonicalize()
                            .unwrap_or_else(|_| clone_root.to_path_buf()),
                    )
                {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
                if !full.is_dir() {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
            }

            // spec: DSC-74 — resolve the effective flat-skills setting: the
            // consumer `--flat-skills` override (STO-44) wins; else the source's
            // own `[source].flat-skills`; else false (the DSC-10 container layout).
            let flat_skills =
                source.flat_skills || mt.as_ref().map(|m| m.source.flat_skills).unwrap_or(false);

            // spec: DSC-53 — scan each root and union the results. Detect a
            // (kind, bare_name) collision within this source.
            let pre_scan_len = out.len();
            for r in &effective_roots {
                let scan_root = clone_root.join(r);
                scan_convention(&scan_root, source, &prefix, flat_skills, out)?;
            }
            // Check for duplicates among items contributed by this source.
            let new_items = &out[pre_scan_len..];
            let mut seen: std::collections::HashSet<(crate::error::ItemKind, String)> =
                std::collections::HashSet::new();
            for item in new_items {
                let key = (item.kind, item.name.clone());
                if !seen.insert(key.clone()) {
                    return Err(MindError::DuplicateItem {
                        source_name: source.name.clone(),
                        kind: key.0,
                        name: key.1,
                    });
                }
            }
            Ok(())
        }
    }
}

/// Build a catalog item from an explicit `[[items]]` declaration.
fn from_decl(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    decl: &ItemDecl,
) -> Result<CatalogItem> {
    let kind = ItemKind::parse(&decl.kind).ok_or_else(|| MindError::MindToml {
        path: root.join("mind.toml"),
        msg: format!("unknown item kind '{}' for '{}'", decl.kind, decl.name),
    })?;
    // DSC-71/DSC-72: a melded source's `name` and `link` flow into filesystem
    // paths (the store key and the per-home symlink), so reject any value that
    // could escape its kind directory or the agent home before it is used.
    if !is_safe_item_name(&decl.name) {
        return Err(MindError::MindToml {
            path: root.join("mind.toml"),
            msg: format!(
                "item name '{}' is unsafe: it must be a single path component (no '/', '\\', \
                 '.', '..', or NUL)",
                decl.name
            ),
        });
    }
    if let Some(link) = &decl.link
        && !is_safe_link_rel(link)
    {
        return Err(MindError::MindToml {
            path: root.join("mind.toml"),
            msg: format!(
                "item '{}' has an unsafe link '{}': it must be a relative path inside the agent \
                 home (no leading '/' or '~', no '..' component, no NUL)",
                decl.name, link
            ),
        });
    }
    // `bin` and `build` describe tooling, so they are valid only on a tool item.
    if kind != ItemKind::Tool && (decl.bin.is_some() || decl.build.is_some()) {
        return Err(MindError::MindToml {
            path: root.join("mind.toml"),
            msg: format!(
                "`bin`/`build` are only valid on a tool item, not '{}' ('{}')",
                decl.kind, decl.name
            ),
        });
    }
    // spec: DSC-73 — a [[items]] `path` must be a safe repo-root-relative path.
    // Reuse `is_safe_link_rel` (the same rule: relative, no `..`, no absolute or
    // `~`-rooted value, no NUL). Without this guard, `root.join` with an absolute
    // operand silently discards `root`, and a `..`-bearing path escapes the clone.
    if !is_safe_link_rel(&decl.path) {
        return Err(MindError::MindToml {
            path: root.join("mind.toml"),
            msg: format!(
                "item '{}' has an unsafe path '{}': must be a relative path inside the clone \
                 (no leading '/' or '~', no '..' component, no NUL)",
                decl.name, decl.path
            ),
        });
    }
    let path = root.join(&decl.path);
    let meta = meta_file(kind, &path);
    // HOOK-86: resolve the item's full lifecycle hook list (scalar shorthand
    // folded ahead of the `[[items.hooks]]` array, validated). This is the
    // authoritative list for the `mind.toml` path; the scalar fields below stay
    // populated for the HOOK-85 disclosure.
    let hooks = decl.resolved_item_hooks(&root.join("mind.toml"))?;
    Ok(build_item(
        source,
        prefix,
        kind,
        decl.name.clone(),
        path,
        &meta,
        ItemOverrides {
            description: decl.description.clone(),
            link: decl.link.clone(),
            bin: decl.bin.clone(),
            build: decl.build.clone(),
            install: decl.install.clone(),
            uninstall: decl.uninstall.clone(),
            hooks: Some(hooks),
        },
    ))
}

/// True when `name` is a single safe path component (DSC-71): non-empty, not `.`
/// or `..`, and free of a path separator or NUL. The name keys the store and the
/// per-home symlink, so anything else could steer those paths out of the kind
/// directory.
fn is_safe_item_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return false;
    }
    // Belt and suspenders: exactly one normal component, nothing else.
    let mut comps = Path::new(name).components();
    matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none()
}

/// True when `rel` is a safe link target relative to an agent home (DSC-72):
/// non-empty, not absolute, not `~`-rooted, and with no parent (`..`)/root/prefix
/// component or NUL. `rel` may contain `/` for subdirectories; it just may not
/// escape the home.
fn is_safe_link_rel(rel: &str) -> bool {
    if rel.is_empty() || rel.contains('\0') || rel.starts_with('~') {
        return false;
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return false;
    }
    use std::path::Component;
    p.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Read a lifecycle hook (`install`/`uninstall`, HOOK-80) from an item's meta
/// file frontmatter, but only for a tool (a `TOOL.md`). Other kinds declare
/// these only via `mind.toml` `[[items]]`, so frontmatter is not consulted.
/// An empty or whitespace-only value is treated as absent (HOOK-3).
fn lifecycle_frontmatter(kind: ItemKind, meta: &Path, key: &str) -> Option<String> {
    if kind != ItemKind::Tool {
        return None;
    }
    nonempty(frontmatter::file_field(meta, key))
}

/// Trim a value and treat an empty/whitespace-only string as absent (HOOK-3).
fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Resolve a tool's `bin`/`build`: an explicit `mind.toml` value wins, else the
/// `TOOL.md` frontmatter value. Always `None` for a non-tool kind.
fn tool_field(kind: ItemKind, explicit: Option<String>, meta: &Path, key: &str) -> Option<String> {
    if kind != ItemKind::Tool {
        return None;
    }
    explicit.or_else(|| frontmatter::file_field(meta, key))
}

/// Field overrides from a `[[items]]` declaration. Every field is empty for
/// convention discovery (`make_item`); a `mind.toml` item supplies the ones it
/// declares (`from_decl`). Each takes precedence over the frontmatter fallback.
#[derive(Default)]
struct ItemOverrides {
    description: Option<String>,
    link: Option<String>,
    bin: Option<String>,
    build: Option<String>,
    install: Option<String>,
    uninstall: Option<String>,
    /// The item's fully resolved lifecycle hooks (HOOK-86) in execution order:
    /// the scalar install/uninstall shorthand folded in ahead of any
    /// `[[items.hooks]]` array entries. `None` lets `build_item` derive the list
    /// from the resolved scalar fields alone (the convention/TOOL.md path, where
    /// there is no array form, DSC-21).
    hooks: Option<Vec<ResolvedHook>>,
}

/// The single `CatalogItem` constructor: it applies the override-then-frontmatter
/// fallback policy once, so convention discovery and `[[items]]` declarations
/// share one definition of how each field is resolved.
fn build_item(
    source: &Source,
    prefix: &Option<String>,
    kind: ItemKind,
    name: String,
    path: PathBuf,
    meta: &Path,
    ov: ItemOverrides,
) -> CatalogItem {
    // HOOK-80: a `mind.toml` install/uninstall is valid on any kind; a tool's
    // TOOL.md may also carry one in frontmatter. An empty value is absent. These
    // scalar fields stay populated for the HOOK-85 disclosure, alongside `hooks`.
    let install = nonempty(ov.install).or_else(|| lifecycle_frontmatter(kind, meta, "install"));
    let uninstall =
        nonempty(ov.uninstall).or_else(|| lifecycle_frontmatter(kind, meta, "uninstall"));
    // HOOK-86: the full resolved hook list in execution order. On the `mind.toml`
    // path the caller supplies it via `ItemDecl::resolved_item_hooks` (scalar
    // shorthand folded ahead of the `[[items.hooks]]` array). On the
    // convention/TOOL.md path there is no array (DSC-21), so derive it from the
    // resolved scalar install/uninstall (which may come from TOOL.md frontmatter),
    // each as one required hook of its event.
    let hooks = ov.hooks.unwrap_or_else(|| {
        let mut out: Vec<ResolvedHook> = Vec::new();
        for (cmd, event) in [
            (&install, HookEvent::Install),
            (&uninstall, HookEvent::Uninstall),
        ] {
            if let Some(c) = cmd {
                out.push(ResolvedHook {
                    run: c.clone(),
                    name: None,
                    optional: false,
                    event,
                });
            }
        }
        out
    });
    // DEP-4: read the `requires:` frontmatter scalar and split on whitespace.
    // This is always read from `meta` regardless of kind; absent or empty -> empty Vec.
    let requires: Vec<String> = frontmatter::file_field(meta, "requires")
        .map(|s| s.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    CatalogItem {
        kind,
        name,
        source: source.name.clone(),
        prefix: prefix.clone(),
        path,
        description: ov.description.or_else(|| frontmatter::description(meta)),
        link_rel: ov.link,
        bin: tool_field(kind, ov.bin, meta, "bin"),
        build: tool_field(kind, ov.build, meta, "build"),
        install,
        uninstall,
        requires,
        hooks,
    }
}

/// Discover items by glob, relative to the repo root. Nested `sources` are
/// handled at meld time, not here.
fn scan_globs(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    discover: &Discover,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    for skill_md in resolve_globs(root, &discover.skills)? {
        // The glob points at the SKILL.md; the item is its parent dir.
        if let Some(dir) = skill_md.parent() {
            out.push(make_item(
                source,
                prefix,
                ItemKind::Skill,
                dir.to_path_buf(),
                &skill_md,
            ));
        }
    }
    for (kind, globs) in [
        (ItemKind::Agent, &discover.agents),
        (ItemKind::Rule, &discover.rules),
    ] {
        for md in resolve_globs(root, globs)? {
            out.push(make_item(source, prefix, kind, md.clone(), &md));
        }
    }
    // Tool globs match the tool directory itself; its `TOOL.md` (if any) is the
    // metadata source.
    for dir in resolve_globs(root, &discover.tools)? {
        let meta = dir.join("TOOL.md");
        out.push(make_item(source, prefix, ItemKind::Tool, dir, &meta));
    }
    Ok(())
}

/// Expand a kind's include globs, then drop anything its exclude globs match.
fn resolve_globs(root: &Path, globs: &KindGlobs) -> Result<Vec<PathBuf>> {
    let mut included = BTreeSet::new();
    for pattern in &globs.include {
        included.extend(glob_paths(root, pattern)?);
    }
    let mut excluded = BTreeSet::new();
    for pattern in &globs.exclude {
        excluded.extend(glob_paths(root, pattern)?);
    }
    Ok(included.difference(&excluded).cloned().collect())
}

/// Convention scan: fixed `skills/`, `agents/`, `rules/` directories.
///
/// When `flat_skills` is true (DSC-74), skills are instead found as bare-name
/// directories with a direct `SKILL.md` immediately under `root` (no `skills/`
/// container); agent, rule, and tool discovery are unchanged either way.
fn scan_convention(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    flat_skills: bool,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    // spec: DSC-74 — flat layout: each immediate child directory of `root` that
    // contains a direct `SKILL.md` is a skill, taking the directory name as its
    // bare name. The scan is shallow (only `root`'s immediate children), and the
    // `SKILL.md` anchor disambiguates a skill dir from `agents/`, `rules/`, etc.
    // Otherwise (DSC-10) skills live under the `skills/` container.
    let skills_dir = if flat_skills {
        root.to_path_buf()
    } else {
        root.join(ItemKind::Skill.dir())
    };
    for entry in read_dir_opt(&skills_dir)? {
        let skill_md = entry.join("SKILL.md");
        if entry.is_dir() && skill_md.is_file() {
            out.push(make_item(source, prefix, ItemKind::Skill, entry, &skill_md));
        }
    }

    for kind in [ItemKind::Agent, ItemKind::Rule] {
        let kind_dir = root.join(kind.dir());
        for entry in read_dir_opt(&kind_dir)? {
            if entry.is_file() && entry.extension().is_some_and(|e| e == "md") {
                out.push(make_item(source, prefix, kind, entry.clone(), &entry));
            }
        }
    }

    // Tools: every immediate subdirectory of `tools/` is a tool. Unlike a skill,
    // a tool needs no anchor file; its directory contents are the tool. An
    // optional `TOOL.md` carries `description`/`bin`/`build` (read in make_item).
    let tools_dir = root.join(ItemKind::Tool.dir());
    for entry in read_dir_opt(&tools_dir)? {
        if entry.is_dir() {
            let meta = entry.join("TOOL.md");
            out.push(make_item(source, prefix, ItemKind::Tool, entry, &meta));
        }
    }
    Ok(())
}

/// Build a [`CatalogItem`], deriving its bare name from the path and its
/// description from `meta_file`'s frontmatter, then applying the prefix.
fn make_item(
    source: &Source,
    prefix: &Option<String>,
    kind: ItemKind,
    path: PathBuf,
    meta: &Path,
) -> CatalogItem {
    let bare = match kind {
        // Directory-shaped items take the directory name; file items the stem.
        ItemKind::Skill | ItemKind::Tool => file_name(&path),
        ItemKind::Agent | ItemKind::Rule => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    // Convention discovery carries no overrides: every field falls back to the
    // item's frontmatter (HOOK-80: install/uninstall only from a tool's TOOL.md).
    build_item(
        source,
        prefix,
        kind,
        bare,
        path,
        meta,
        ItemOverrides::default(),
    )
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmp() -> PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("mind-lifecycle-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn source_for(clone: &Path) -> Source {
        use crate::source::Pin;
        Source {
            name: "local/test/repo".to_string(),
            url: clone.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: "repo".to_string(),
            commit: None,
            description: None,
            alias: None,
            pin: Pin::default(),
            roots: None,
            flat_skills: false,
            install_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
        }
    }

    #[test]
    fn item_install_uninstall_hooks_from_mind_toml_on_any_kind() {
        // spec: HOOK-80
        // A `mind.toml` [[items]].install/.uninstall is valid on a non-tool kind
        // (here a rule), unlike `bin`/`build` which are tool-only.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style\n---\n# style\n",
        );
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
                "install = \"echo set-up\"\n",
                "uninstall = \"echo tear-down\"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let rule = items.iter().find(|i| i.name == "style").unwrap();
        assert_eq!(rule.install.as_deref(), Some("echo set-up"));
        assert_eq!(rule.uninstall.as_deref(), Some("echo tear-down"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn item_hooks_from_tool_md_frontmatter() {
        // spec: HOOK-80
        // A tool's TOOL.md may carry install:/uninstall: in frontmatter.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("tools/helper/TOOL.md"),
            "---\ndescription: helper\ninstall: make setup\nuninstall: make cleanup\n---\n# helper\n",
        );
        write(&clone.join("tools/helper/helper"), "#!/bin/sh\n");
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let tool = items.iter().find(|i| i.name == "helper").unwrap();
        assert_eq!(tool.install.as_deref(), Some("make setup"));
        assert_eq!(tool.uninstall.as_deref(), Some("make cleanup"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_item_hook_is_treated_as_absent() {
        // spec: HOOK-80
        // An empty/whitespace install or uninstall is absent (HOOK-3).
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style\n---\n# style\n",
        );
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
                "install = \"   \"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let rule = items.iter().find(|i| i.name == "style").unwrap();
        assert_eq!(rule.install, None, "whitespace install must be absent");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scalar_item_hooks_populate_both_the_scalar_fields_and_the_list() {
        // spec: HOOK-86
        // COORDINATION: the scalar install/uninstall fields stay populated (the
        // HOOK-85 disclosure reads them) AND the resolved hook list is populated.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style\n---\n# style\n",
        );
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
                "install = \"echo set-up\"\n",
                "uninstall = \"echo tear-down\"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let rule = items.iter().find(|i| i.name == "style").unwrap();
        // Scalar fields still populated.
        assert_eq!(rule.install.as_deref(), Some("echo set-up"));
        assert_eq!(rule.uninstall.as_deref(), Some("echo tear-down"));
        // The resolved list mirrors them: one required install, one required
        // uninstall, in fold-in order.
        assert_eq!(rule.hooks.len(), 2);
        let ih = rule.install_hooks();
        assert_eq!(ih.len(), 1);
        assert_eq!(ih[0].run, "echo set-up");
        let uh = rule.uninstall_hooks();
        assert_eq!(uh.len(), 1);
        assert_eq!(uh[0].run, "echo tear-down");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn array_item_hooks_resolve_in_order_with_scalar_folded_ahead() {
        // spec: HOOK-86
        // A `[[items.hooks]]` array plus a scalar install: the scalar folds in as
        // the first install hook, then the array entries in declaration order.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(&clone.join("tools/helper/helper"), "#!/bin/sh\n");
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"tool\"\n",
                "name = \"helper\"\n",
                "path = \"tools/helper\"\n",
                "install = \"scalar-install\"\n",
                "\n",
                "[[items.hooks]]\n",
                "run = \"array-install\"\n",
                "name = \"Second step\"\n",
                "\n",
                "[[items.hooks]]\n",
                "run = \"array-uninstall\"\n",
                "event = \"uninstall\"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let tool = items.iter().find(|i| i.name == "helper").unwrap();
        // Scalar field still set.
        assert_eq!(tool.install.as_deref(), Some("scalar-install"));
        // Full list: scalar install, then the two array entries.
        assert_eq!(tool.hooks.len(), 3);
        let ih = tool.install_hooks();
        assert_eq!(ih.len(), 2);
        assert_eq!(ih[0].run, "scalar-install");
        assert_eq!(ih[1].run, "array-install");
        assert_eq!(ih[1].name.as_deref(), Some("Second step"));
        let uh = tool.uninstall_hooks();
        assert_eq!(uh.len(), 1);
        assert_eq!(uh[0].run, "array-uninstall");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tool_md_scalar_hooks_fold_into_the_list() {
        // spec: HOOK-86
        // For a convention-discovered tool, the TOOL.md install:/uninstall:
        // frontmatter scalars (DSC-21: the only form there) fold into the hook
        // list AND populate the scalar fields.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("tools/helper/TOOL.md"),
            "---\ndescription: helper\ninstall: make setup\nuninstall: make cleanup\n---\n# helper\n",
        );
        write(&clone.join("tools/helper/helper"), "#!/bin/sh\n");
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let tool = items.iter().find(|i| i.name == "helper").unwrap();
        assert_eq!(tool.install.as_deref(), Some("make setup"));
        assert_eq!(tool.uninstall.as_deref(), Some("make cleanup"));
        // Folded into the list as one required hook each.
        assert_eq!(tool.hooks.len(), 2);
        assert_eq!(tool.install_hooks()[0].run, "make setup");
        assert!(!tool.install_hooks()[0].optional);
        assert_eq!(tool.uninstall_hooks()[0].run, "make cleanup");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn item_array_hooks_unknown_event_is_a_scan_error() {
        // spec: HOOK-86
        // An unknown event in a `[[items.hooks]]` entry surfaces as a mind.toml
        // schema error from the scan (via from_decl).
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(&clone.join("tools/helper/helper"), "#!/bin/sh\n");
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"tool\"\n",
                "name = \"helper\"\n",
                "path = \"tools/helper\"\n",
                "\n",
                "[[items.hooks]]\n",
                "run = \"do-it\"\n",
                "event = \"build\"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        let err = scan_source(&paths, &source_for(&clone), &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "unknown item hook event must be a schema error: {err}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn requires_populated_on_authoritative_mind_toml_item() {
        // spec: DEP-4
        // An authoritative `[[items]]` declaration routes through `from_decl` ->
        // `build_item`, the same constructor as convention discovery. So an item
        // declared in mind.toml whose META FILE frontmatter carries `requires:`
        // must still have that field populated (it is read from the meta file, not
        // from the `[[items]]` table). Pins the otherwise-untested mind.toml route.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style\nrequires: agent:linter\n---\n# style\n",
        );
        write(
            &clone.join("agents/linter.md"),
            "---\ndescription: linter\n---\n# linter\n",
        );
        write(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
                "[[items]]\n",
                "kind = \"agent\"\n",
                "name = \"linter\"\n",
                "path = \"agents/linter.md\"\n",
            ),
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let rule = items.iter().find(|i| i.name == "style").unwrap();
        assert_eq!(
            rule.requires,
            vec!["agent:linter".to_string()],
            "requires from the meta-file frontmatter must populate on the authoritative mind.toml path"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn requires_splits_on_arbitrary_whitespace() {
        // spec: DEP-4
        // The `requires:` scalar is split on whitespace, not a YAML sequence:
        // multiple internal spaces and leading/trailing whitespace collapse to a
        // clean list of entries (DEP-4: "a single string split on whitespace").
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\nrequires:   agent:a    rule:b  \n---\n# review\n",
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let skill = items.iter().find(|i| i.name == "review").unwrap();
        assert_eq!(
            skill.requires,
            vec!["agent:a".to_string(), "rule:b".to_string()],
            "extra/leading/trailing whitespace must split into exactly two entries"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_requires_scalar_yields_no_entries() {
        // spec: DEP-4
        // An empty (or whitespace-only) `requires:` value yields an empty entry
        // list and no error: `"".split_whitespace()` produces zero items. Pins
        // that an author writing `requires:` with no value is a benign no-op, not
        // a spurious edge or a bad-reference at scan time.
        let base = tmp();
        let clone = base.join("sources/local/test/repo");
        write(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\nrequires:    \n---\n# review\n",
        );
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        let mut items = Vec::new();
        scan_source(&paths, &source_for(&clone), &mut items).unwrap();
        let skill = items.iter().find(|i| i.name == "review").unwrap();
        assert!(
            skill.requires.is_empty(),
            "a whitespace-only requires value must yield no entries: {:?}",
            skill.requires
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// The file whose frontmatter describes an item (SKILL.md for a skill, TOOL.md
/// for a tool, the item file itself for an agent/rule). The file may be absent
/// for a tool (it is optional), in which case frontmatter reads yield `None`.
fn meta_file(kind: ItemKind, path: &Path) -> PathBuf {
    match kind {
        ItemKind::Skill => path.join("SKILL.md"),
        ItemKind::Tool => path.join("TOOL.md"),
        ItemKind::Agent | ItemKind::Rule => path.to_path_buf(),
    }
}

/// Expand a glob pattern rooted at `root`, returning sorted matches.
fn glob_paths(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let joined = root.join(pattern);
    let full = joined.to_string_lossy();
    let paths = glob::glob(&full).map_err(|e| MindError::MindToml {
        path: root.join("mind.toml"),
        msg: format!("bad discover glob '{pattern}': {e}"),
    })?;
    let mut out = Vec::new();
    for entry in paths {
        match entry {
            Ok(p) => out.push(p),
            Err(e) => {
                let path = e.path().to_path_buf();
                return Err(MindError::io(path, e.into_error()));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Read a directory's entries, treating "not found" as empty.
fn read_dir_opt(dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut paths = Vec::new();
            for entry in rd {
                let entry = entry.map_err(|e| MindError::io(dir, e))?;
                paths.push(entry.path());
            }
            paths.sort();
            Ok(paths)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(MindError::io(dir, e)),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use crate::paths::Paths;
    use crate::source::{Pin, Source};
    use std::path::PathBuf;

    // ---- scan roots unit tests (DSC-50, DSC-51, DSC-52, DSC-53) -------

    use std::sync::atomic::{AtomicU32, Ordering};
    static UNIT_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Allocate a unique temp dir for a unit test and return a guard that
    /// removes it on drop (via a wrapper struct).
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let n = UNIT_COUNTER.fetch_add(1, Ordering::SeqCst);
            let p =
                std::env::temp_dir().join(format!("mind-catalog-unit-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Create a minimal `Source` for a local path fixture.
    fn make_source_for(clone: &std::path::Path) -> Source {
        Source {
            name: "local/test/repo".to_string(),
            url: clone.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: "repo".to_string(),
            commit: None,
            description: None,
            alias: None,
            pin: Pin::default(),
            roots: None,
            flat_skills: false,
            install_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
        }
    }

    /// Create a `Paths` whose sources dir is `base/sources`, so that
    /// the clone of `local/test/repo` lives at `base/sources/local/test/repo`.
    fn paths_for(base: &std::path::Path) -> Paths {
        Paths {
            mind_home: base.to_path_buf(),
            claude_home: base.join("claude"),
        }
    }

    fn write_file(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn convention_discovery_under_single_explicit_root() {
        // spec: DSC-50 DSC-53
        // When [source].roots = ["tools"], items in tools/skills/ etc. are found.
        let tmp = TmpDir::new();
        let base = tmp.path();

        // The clone lands at base/sources/local/test/repo.
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/meld/SKILL.md"),
            "---\ndescription: meld skill\n---\n# meld\n",
        );
        write_file(
            &clone.join("tools/agents/do.md"),
            "---\ndescription: do agent\n---\n# do\n",
        );
        // Write a mind.toml with roots = ["tools"].
        write_file(&clone.join("mind.toml"), "[source]\nroots = [\"tools\"]\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"meld"), "expected 'meld': {names:?}");
        assert!(names.contains(&"do"), "expected 'do': {names:?}");
        // No items from the repo root (no skills/ at root).
        assert!(!names.contains(&"review"), "unexpected 'review': {names:?}");
    }

    #[test]
    fn source_roots_override_beats_mindfile_roots() {
        // spec: DSC-51 STO-17
        // Source.roots (--root override) takes precedence over [source].roots in mind.toml.
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // Items only under "a/".
        write_file(
            &clone.join("a/skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );
        // Items only under "b/".
        write_file(
            &clone.join("b/skills/beta/SKILL.md"),
            "---\ndescription: beta\n---\n# beta\n",
        );
        // mind.toml says roots = ["b"], but the source override says ["a"].
        write_file(&clone.join("mind.toml"), "[source]\nroots = [\"b\"]\n");

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // Consumer --root override points at "a".
        source.roots = Some(vec!["a".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"alpha"),
            "override root 'a' expected: {names:?}"
        );
        assert!(
            !names.contains(&"beta"),
            "toml root 'b' should be ignored: {names:?}"
        );
    }

    #[test]
    fn two_roots_are_unioned() {
        // spec: DSC-53
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("a/skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );
        write_file(
            &clone.join("b/skills/beta/SKILL.md"),
            "---\ndescription: beta\n---\n# beta\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "expected alpha: {names:?}");
        assert!(names.contains(&"beta"), "expected beta: {names:?}");
    }

    #[test]
    fn duplicate_item_across_roots_is_an_error() {
        // spec: DSC-53
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // "review" skill under both "a/" and "b/".
        write_file(
            &clone.join("a/skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        write_file(
            &clone.join("b/skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "review"),
            "expected DuplicateItem: {err}"
        );
    }

    #[test]
    fn flat_skills_discovers_bare_dirs_and_composes_with_roots() {
        // spec: DSC-74
        // With flat_skills set, a skill is a bare-name directory containing a
        // direct SKILL.md under each scan root (no `skills/` container). The
        // SKILL.md anchor disambiguates a skill dir from an arbitrary one, and
        // agent discovery (a conventional `agents/` dir) is unchanged. It composes
        // with roots: here a single root `pkg`.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        // Flat skills directly under the `pkg` root.
        write_file(
            &clone.join("pkg/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );
        write_file(
            &clone.join("pkg/beta/SKILL.md"),
            "---\ndescription: beta\n---\n# beta\n",
        );
        // A bare dir with no SKILL.md must NOT be classified as a skill.
        write_file(&clone.join("pkg/notaskill/README.md"), "# nope\n");
        // Agent discovery under the same root is unchanged.
        write_file(
            &clone.join("pkg/agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["pkg".to_string()]);
        source.flat_skills = true;

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let skills: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == ItemKind::Skill)
            .map(|i| i.name.as_str())
            .collect();
        assert!(
            skills.contains(&"alpha"),
            "expected flat skill alpha: {skills:?}"
        );
        assert!(
            skills.contains(&"beta"),
            "expected flat skill beta: {skills:?}"
        );
        assert!(
            !skills.contains(&"notaskill"),
            "a dir without SKILL.md must not be a skill: {skills:?}"
        );
        // The agent is still discovered conventionally.
        assert!(
            items
                .iter()
                .any(|i| i.kind == ItemKind::Agent && i.name == "dev"),
            "agent discovery must be unchanged under flat-skills"
        );
    }

    #[test]
    fn flat_skills_off_requires_skills_container() {
        // spec: DSC-74
        // With flat_skills false (the default), a bare-name skill dir at the root
        // is NOT discovered; the `skills/` container is required (DSC-10).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone); // flat_skills defaults false
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        assert!(
            items.is_empty(),
            "a root-level skill dir must not be found without flat-skills: {:?}",
            items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn flat_skills_duplicate_across_roots_is_an_error() {
        // spec: DSC-74 DSC-53
        // Flat discovery composes with multi-root union and the within-source
        // uniqueness check: two roots each shipping a flat `alpha/SKILL.md` is a
        // DuplicateItem, exactly as for the containered layout.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("a/alpha/SKILL.md"),
            "---\ndescription: alpha a\n---\n# alpha\n",
        );
        write_file(
            &clone.join("b/alpha/SKILL.md"),
            "---\ndescription: alpha b\n---\n# alpha\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);
        source.flat_skills = true;

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "alpha"),
            "expected DuplicateItem for a flat skill across two roots: {err}"
        );
    }

    #[test]
    fn non_directory_root_is_invalid_root_error() {
        // spec: DSC-52
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["nonexistent".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "nonexistent"),
            "expected InvalidRoot: {err}"
        );
    }

    #[test]
    fn authoritative_mind_toml_ignores_roots() {
        // spec: DSC-52
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // A rule declared explicitly in mind.toml.
        write_file(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style rule\n---\n# style\n",
        );
        // Convention items under "sub/".
        write_file(
            &clone.join("sub/skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );
        write_file(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
            ),
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // Consumer root pointing at "sub/" -- should be ignored for authoritative source.
        source.roots = Some(vec!["sub".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        // Only the explicitly declared item; the convention root is ignored.
        assert!(names.contains(&"style"), "expected 'style': {names:?}");
        assert!(
            !names.contains(&"review"),
            "convention scan should be ignored: {names:?}"
        );
    }

    #[test]
    fn absolute_root_pointing_inside_the_clone_is_still_invalid() {
        // spec: DSC-52 CLI-16
        // A root must be repo-root-relative. An ABSOLUTE path is rejected even
        // when it names a real directory INSIDE the clone -- so only the
        // is_absolute() guard can catch it (the escape and is_dir checks would
        // both pass). This isolates the absolute-path guard from the escape guard.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/build/SKILL.md"),
            "---\ndescription: build\n---\n# build\n",
        );
        // The clone path must canonicalize stably (no symlinks in temp here).
        let abs_inside = clone.join("tools").canonicalize().unwrap();
        assert!(abs_inside.is_absolute());

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec![abs_inside.to_string_lossy().into_owned()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { .. }),
            "an absolute root, even inside the clone, must be InvalidRoot: {err}"
        );
        assert!(items.is_empty(), "absolute root must contribute nothing");
    }

    #[test]
    fn absolute_root_outside_the_clone_is_invalid() {
        // spec: DSC-52 CLI-16
        // The plain case: an absolute path outside the clone is rejected.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["/tmp".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "/tmp"),
            "absolute root outside the clone must be InvalidRoot: {err}"
        );
    }

    #[test]
    fn parent_escaping_root_to_existing_sibling_is_invalid_root() {
        // spec: DSC-52 CLI-16
        // The escape guard must reject a `..` root that resolves to a real
        // directory OUTSIDE the clone. This is the adversarial case the is_dir()
        // check alone cannot catch (the sibling exists), so only the
        // canonicalize/starts_with guard stands between it and a read outside the
        // clone.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        // A sibling clone dir that exists and even has scannable items.
        let sibling = base.join("sources/local/test/other");
        write_file(
            &sibling.join("skills/leak/SKILL.md"),
            "---\ndescription: leaked\n---\n# leak\n",
        );
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // ../other escapes the clone but points at an existing directory.
        source.roots = Some(vec!["../other".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "../other"),
            "escaping root must be InvalidRoot, not a silent read outside the clone: {err}"
        );
        assert!(
            items.is_empty(),
            "no items should leak from outside the clone"
        );
    }

    #[test]
    fn in_clone_dotdot_root_is_allowed() {
        // spec: DSC-50 DSC-52
        // A `..` segment that stays inside the clone (`tools/../tools`) is a
        // legitimate in-clone path and must be accepted, distinguishing it from a
        // genuinely escaping `../x`. Mirror test of the escape rejection: this
        // pins that the guard is not over-broad (rejecting all `..`).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/build/SKILL.md"),
            "---\ndescription: build\n---\n# build\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["tools/../tools".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"build"),
            "in-clone .. should resolve: {names:?}"
        );
    }

    #[test]
    fn duplicate_item_check_is_scoped_to_one_source() {
        // spec: DSC-53
        // The (kind, bare_name) duplicate check is per-source: a `review` skill in
        // source A and a `review` skill in source B is NOT a DuplicateItem -- only
        // a collision WITHIN one source's roots is. Regression guard: if the dedup
        // scanned `out` from index 0 instead of this source's slice, this would
        // wrongly error.
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone_a = base.join("sources/local/test/repo");
        write_file(
            &clone_a.join("skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        let clone_b = base.join("sources/local/other/repo");
        write_file(
            &clone_b.join("skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source_a = make_source_for(&clone_a);
        let mut source_b = make_source_for(&clone_b);
        source_b.name = "local/other/repo".to_string();
        source_b.owner = "other".to_string();

        let mut items = Vec::new();
        scan_source(&paths, &source_a, &mut items).unwrap();
        // Scanning B into the same `out` that already holds A's `review` must not
        // be seen as a duplicate.
        scan_source(&paths, &source_b, &mut items)
            .expect("same name in a different source is not a DuplicateItem");
        let reviews = items.iter().filter(|i| i.name == "review").count();
        assert_eq!(reviews, 2, "both sources' review items should be present");
    }

    #[test]
    fn duplicate_across_roots_collides_on_bare_name_under_a_prefix() {
        // spec: DSC-53
        // The duplicate check is on the BARE name, independent of any namespace
        // prefix: two roots each contributing a bare `review` collide even when
        // the source has a prefix/alias (which would prefix both identically).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("a/skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        write_file(
            &clone.join("b/skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.alias = Some("jk".to_string()); // a namespace prefix is in effect
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "review"),
            "bare-name collision must error regardless of prefix: {err}"
        );
    }

    #[test]
    fn explicit_empty_roots_list_discovers_nothing() {
        // spec: DSC-50
        // DSC-50 says an UNSET `roots` means a single implicit repo root. An
        // explicitly empty list (`roots = []`) is distinct: it is honored as
        // "scan zero roots", so nothing is discovered. This pins the
        // unset-vs-explicit-empty fork rather than letting [] silently fall back
        // to the repo root. See certification note (spec ambiguity).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        // A conventional item at the repo root: it WOULD be found by the implicit
        // root, so if [] fell back to the repo root this item would appear.
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );
        write_file(&clone.join("mind.toml"), "[source]\nroots = []\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        assert!(
            items.is_empty(),
            "an explicit empty roots list scans zero roots: {:?}",
            items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unset_roots_falls_back_to_implicit_repo_root() {
        // spec: DSC-50
        // The counterpart to the empty-list case: with no roots configured at all,
        // discovery scans the repo root (the DSC-10..13 behavior). This is the
        // mutation guard distinguishing `None` (implicit ["."]) from `Some([])`.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone); // roots: None, no mind.toml

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"review"),
            "unset roots scans the repo root: {names:?}"
        );
    }

    fn make_test_item(name: &str, description: Option<&str>) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            source: "test-source".to_string(),
            prefix: None,
            path: PathBuf::from("/tmp/fake"),
            description: description.map(|s| s.to_string()),
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            hooks: Vec::new(),
        }
    }

    #[test]
    fn convention_discovers_bare_tool_dir_without_anchor() {
        // spec: TOOL-1 TOOL-5
        // A `tools/<name>/` directory is a tool with no anchor file; the
        // convention default entrypoint is a file named after the tool.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(&clone.join("tools/detect/detect"), "#!/bin/sh\necho hi\n");
        write_file(&clone.join("tools/detect/lib.sh"), "helper\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let tool = items
            .iter()
            .find(|i| i.name == "detect")
            .expect("tool 'detect' discovered");
        assert_eq!(tool.kind, ItemKind::Tool);
        assert_eq!(tool.resolved_bin().as_deref(), Some("detect"));
    }

    #[test]
    fn tool_metadata_comes_from_optional_tool_md() {
        // spec: TOOL-2 TOOL-5 HOOK-70
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/shard/TOOL.md"),
            "---\ndescription: shard a plan\nbin: shard.py\nbuild: make shard\n---\n# shard\n",
        );
        write_file(&clone.join("tools/shard/shard.py"), "print('x')\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let tool = items.iter().find(|i| i.name == "shard").unwrap();
        assert_eq!(tool.description.as_deref(), Some("shard a plan"));
        // An explicit `bin:` wins over the convention default.
        assert_eq!(tool.resolved_bin().as_deref(), Some("shard.py"));
        // HOOK-70: the per-item build command is read from TOOL.md frontmatter.
        assert_eq!(tool.build.as_deref(), Some("make shard"));
    }

    #[test]
    fn resolved_bin_convention_default_requires_the_file() {
        // spec: TOOL-5
        // With no declared bin and no `tools/<name>/<name>` file present, there is
        // no resolvable entrypoint.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let dir = base.join("tools/empty");
        std::fs::create_dir_all(&dir).unwrap();
        let item = CatalogItem {
            kind: ItemKind::Tool,
            name: "empty".to_string(),
            source: "s".to_string(),
            prefix: None,
            path: dir,
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            hooks: Vec::new(),
        };
        assert_eq!(item.resolved_bin(), None);
    }

    #[test]
    fn is_safe_item_name_rejects_traversal_and_separators() {
        // spec: DSC-71
        for ok in ["x", "my-skill", "a.b", "review2"] {
            assert!(is_safe_item_name(ok), "{ok:?} should be accepted");
        }
        for bad in ["", ".", "..", "a/b", "../x", "/etc", "a\\b", "x\0y"] {
            assert!(!is_safe_item_name(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn is_safe_link_rel_rejects_escape() {
        // spec: DSC-72
        for ok in ["rules/x.md", "skills/x", "commands/x.toml", "./a/b.md"] {
            assert!(is_safe_link_rel(ok), "{ok:?} should be accepted");
        }
        for bad in [
            "",
            "../../.bashrc",
            "/etc/passwd",
            "~/x",
            "a/../../b",
            "x\0y",
        ] {
            assert!(!is_safe_link_rel(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn from_decl_rejects_unsafe_name() {
        // spec: DSC-71
        let tmp = TmpDir::new();
        let root = tmp.path();
        let source = make_source_for(root);
        let decl = ItemDecl {
            kind: "rule".to_string(),
            name: "../../evil".to_string(),
            path: "rules/x.md".to_string(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        let err = from_decl(root, &source, &None, &decl).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "an unsafe item name must be a schema error: {err}"
        );
    }

    #[test]
    fn from_decl_rejects_escaping_link() {
        // spec: DSC-72
        let tmp = TmpDir::new();
        let root = tmp.path();
        let source = make_source_for(root);
        let decl = ItemDecl {
            kind: "rule".to_string(),
            name: "x".to_string(),
            path: "rules/x.md".to_string(),
            link: Some("../../.bashrc".to_string()),
            description: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        let err = from_decl(root, &source, &None, &decl).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "an escaping link override must be a schema error: {err}"
        );
    }

    #[test]
    fn from_decl_rejects_bin_or_build_on_non_tool() {
        // spec: TOOL-7
        let tmp = TmpDir::new();
        let root = tmp.path();
        write_file(&root.join("skills/x/SKILL.md"), "---\n---\n# x\n");
        let source = make_source_for(root);
        let decl = ItemDecl {
            kind: "skill".to_string(),
            name: "x".to_string(),
            path: "skills/x".to_string(),
            link: None,
            description: None,
            bin: Some("x".to_string()),
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        let err = from_decl(root, &source, &None, &decl).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "bin on a non-tool must be a schema error: {err}"
        );
    }

    #[test]
    fn discover_tools_glob_matches_the_directory() {
        // spec: TOOL-7
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(&clone.join("pkgs/detect/tool/detect"), "#!/bin/sh\n");
        write_file(
            &clone.join("mind.toml"),
            "[discover]\ntools = { include = [\"pkgs/*/tool\"] }\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let tool = items.iter().find(|i| i.name == "tool").unwrap();
        assert_eq!(tool.kind, ItemKind::Tool);
    }

    #[test]
    fn empty_query_matches_all() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(matches_query(&item, ""));
    }

    #[test]
    fn matches_by_effective_name() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(matches_query(&item, "review"));
    }

    #[test]
    fn matches_by_description_when_name_does_not_contain_query() {
        // spec: CLI-85
        // "bugs" is only in the description, not the name
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(!item.effective_name().contains("bugs"));
        assert!(matches_query(&item, "bugs"));
    }

    #[test]
    fn match_is_case_insensitive_on_name() {
        // spec: CLI-85
        let item = make_test_item("Review", None);
        assert!(matches_query(&item, "REVIEW"));
        assert!(matches_query(&item, "review"));
        assert!(matches_query(&item, "ReViEw"));
    }

    #[test]
    fn match_is_case_insensitive_on_description() {
        // spec: CLI-85
        let item = make_test_item("x", Some("Implements a Spec with Tests"));
        assert!(matches_query(&item, "SPEC"));
        assert!(matches_query(&item, "spec"));
    }

    #[test]
    fn no_match_when_query_absent_from_both_name_and_description() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(!matches_query(&item, "python"));
    }

    #[test]
    fn no_match_when_description_is_none_and_name_does_not_match() {
        // spec: CLI-85
        let item = make_test_item("review", None);
        assert!(!matches_query(&item, "bugs"));
    }

    #[test]
    fn empty_description_does_not_match_a_nonempty_query() {
        // spec: CLI-85
        // Some("") is distinct from None: an empty description string must not
        // spuriously match a non-empty query (it would if `contains` were
        // reasoned about backwards). The empty *query* still matches (all),
        // but a non-empty query against an empty description must not.
        let item = make_test_item("x", Some(""));
        assert!(matches_query(&item, ""));
        assert!(!matches_query(&item, "anything"));
    }

    #[test]
    fn whitespace_query_matches_a_description_that_contains_whitespace() {
        // spec: CLI-85
        // A non-empty query is a raw substring; it is not trimmed. A query of a
        // single space matches a description containing a space but a name that
        // has none.
        let item = make_test_item("review", Some("Review the diff"));
        assert!(!item.effective_name().contains(' '));
        assert!(matches_query(&item, " "));
    }

    #[test]
    fn substring_in_middle_of_word_matches() {
        // spec: CLI-85
        // Matching is substring, not word-boundary: a fragment inside a longer
        // word matches both in the name and in the description.
        let by_name = make_test_item("refactor", None);
        assert!(matches_query(&by_name, "factor"));
        let by_desc = make_test_item("x", Some("Performs refactoring"));
        assert!(matches_query(&by_desc, "factor"));
    }

    #[test]
    fn prefix_is_used_in_effective_name_match() {
        // spec: CLI-85
        let mut item = make_test_item("review", None);
        item.prefix = Some("jk".to_string());
        // effective_name() is "jk:review"
        assert!(matches_query(&item, "jk:review"));
        assert!(matches_query(&item, "jk"));
        // "review" is a substring of "jk:review", so it also matches
        assert!(matches_query(&item, "review"));
    }

    // ---- DEP-4: `requires:` frontmatter field populated from scan ----------

    #[test]
    fn requires_field_parsed_from_skill_frontmatter() {
        // spec: DEP-4
        // A `requires:` key in SKILL.md is read as a whitespace-split Vec.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\nrequires: skill:plan agent:test\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let skill = items.iter().find(|i| i.name == "review").unwrap();
        assert_eq!(
            skill.requires,
            vec!["skill:plan".to_string(), "agent:test".to_string()],
            "requires must be whitespace-split from the frontmatter scalar"
        );
    }

    #[test]
    fn requires_field_absent_is_empty_vec() {
        // spec: DEP-4
        // When `requires:` is not present, the field is an empty Vec.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let skill = items.iter().find(|i| i.name == "review").unwrap();
        assert!(
            skill.requires.is_empty(),
            "absent requires must yield empty Vec"
        );
    }

    #[test]
    fn requires_field_parsed_from_agent_frontmatter() {
        // spec: DEP-4
        // `requires:` works on an agent file, not just skills.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("agents/dev.md"),
            "---\ndescription: dev\nrequires: rule:style\n---\n# dev\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let agent = items.iter().find(|i| i.name == "dev").unwrap();
        assert_eq!(agent.requires, vec!["rule:style".to_string()],);
    }

    // ---- DSC-73: [[items]] path traversal guard ----------------------------

    #[test]
    fn from_decl_rejects_dotdot_path() {
        // spec: DSC-71 DSC-72 DSC-73
        // A [[items]] `path` with a `..` component must be rejected as MindToml
        // before root.join() can escape the clone. Without the guard,
        // root.join("../escape") silently resolves outside the clone.
        let tmp = TmpDir::new();
        let root = tmp.path();
        let source = make_source_for(root);
        let decl = crate::mindfile::ItemDecl {
            kind: "rule".to_string(),
            name: "evil".to_string(),
            path: "../escape".to_string(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        let err = from_decl(root, &source, &None, &decl).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "a dotdot path must be a schema error: {err}"
        );
    }

    #[test]
    fn from_decl_rejects_absolute_path() {
        // spec: DSC-71 DSC-72 DSC-73
        // An absolute `path` (e.g. "/etc/passwd") is rejected as MindToml.
        // Rust's Path::join with an absolute operand discards `root` entirely,
        // so without this guard the install would copy from an arbitrary host path.
        let tmp = TmpDir::new();
        let root = tmp.path();
        let source = make_source_for(root);
        let decl = crate::mindfile::ItemDecl {
            kind: "rule".to_string(),
            name: "evil".to_string(),
            path: "/etc/passwd".to_string(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        let err = from_decl(root, &source, &None, &decl).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "an absolute path must be a schema error: {err}"
        );
    }

    #[test]
    fn from_decl_accepts_subdir_path() {
        // spec: DSC-73
        // A path with in-bounds subdirectories is accepted; subdirectories are
        // legitimate (a source can organize items below the repo root).
        let tmp = TmpDir::new();
        let root = tmp.path();
        let source = make_source_for(root);
        let decl = crate::mindfile::ItemDecl {
            kind: "rule".to_string(),
            name: "style".to_string(),
            path: "sub/dir/style.md".to_string(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            hooks: Vec::new(),
        };
        // The file need not exist; frontmatter reads return None for absent files.
        let item = from_decl(root, &source, &None, &decl).unwrap();
        assert_eq!(item.name, "style");
        assert_eq!(item.path, root.join("sub/dir/style.md"));
    }

    // ---- DSC-53 (authoritative branch): [[items]] duplicate guard ----------

    #[test]
    fn authoritative_mind_toml_duplicate_items_is_duplicate_item_error() {
        // spec: DSC-53
        // Two [[items]] entries with the same kind+name in one mind.toml must
        // be a DuplicateItem error, enforcing the (source, kind, bare_name)
        // identity invariant in the authoritative branch.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("rules/style.md"),
            "---\ndescription: style\n---\n",
        );
        write_file(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"rules/style.md\"\n",
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"rules/style.md\"\n",
            ),
        );
        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "style"),
            "duplicate [[items]] entries must be DuplicateItem: {err}"
        );
    }

    // ---- agent_harness_name tests (NS-40) ----

    /// Build a minimal `CatalogItem` pointing at a given file for testing
    /// `agent_harness_name()`.
    fn agent_item(path: std::path::PathBuf, bare_name: &str) -> CatalogItem {
        CatalogItem {
            source: "src".to_string(),
            kind: ItemKind::Agent,
            name: bare_name.to_string(),
            prefix: None,
            path,
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            hooks: Vec::new(),
        }
    }

    #[test]
    fn agent_harness_name_reads_frontmatter_name() {
        // spec: NS-40 -- the harness name comes from the frontmatter `name:` field,
        // not the file stem.
        let dir = TmpDir::new();
        let p = dir.path().join("agents/coder.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "---\nname: dev\ndescription: d\n---\n# dev\n").unwrap();
        let item = agent_item(p, "coder");
        // bare catalog name is "coder", but frontmatter says "dev".
        assert_eq!(item.agent_harness_name(), Some("dev".to_string()));
    }

    #[test]
    fn agent_harness_name_falls_back_to_bare_name_when_frontmatter_absent() {
        // spec: NS-40 -- if there is no frontmatter name, fall back to the bare
        // catalog name (file stem).
        let dir = TmpDir::new();
        let p = dir.path().join("agents/coder.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "---\ndescription: d\n---\n# coder\n").unwrap();
        let item = agent_item(p, "coder");
        assert_eq!(item.agent_harness_name(), Some("coder".to_string()));
    }

    #[test]
    fn agent_harness_name_rejects_unsafe_frontmatter_name() {
        // spec: NS-40 -- a frontmatter `name:` that is not a safe path component
        // is ignored and the bare catalog name is used instead.
        let dir = TmpDir::new();
        let p = dir.path().join("agents/coder.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "---\nname: ../evil\ndescription: d\n---\n# coder\n").unwrap();
        let item = agent_item(p, "coder");
        // unsafe name is rejected; falls back to catalog name.
        assert_eq!(item.agent_harness_name(), Some("coder".to_string()));
    }

    #[test]
    fn agent_harness_name_returns_none_for_non_agents() {
        // spec: NS-40 -- only the Agent kind has a harness name.
        let dir = TmpDir::new();
        let p = dir.path().join("skills/review/SKILL.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "---\nname: review\n---\n").unwrap();
        let mut item = agent_item(p, "review");
        item.kind = ItemKind::Skill;
        assert_eq!(item.agent_harness_name(), None);
    }
}
