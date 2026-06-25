//! Author-side source validation: `mind review <target>`.
//!
//! Validates a source before it is published or melded, surfacing every problem
//! that would otherwise only appear at meld/install time. Read-only; installs
//! nothing and changes nothing on disk.
//!
//! spec: CLI-130, CLI-131, CLI-132, CLI-133

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::catalog::{CatalogItem, scan_source_at};
use crate::error::{MindError, Result};
use crate::git;
use crate::mindfile::MindToml;
use crate::paths::{self, Paths};
use crate::resolve::source_matches;
use crate::source::{Registry, Source, parse_spec};

/// A single finding from a review run.
#[derive(Debug)]
pub struct Finding {
    /// Machine-stable tag.
    pub kind: &'static str,
    /// Human-readable message.
    pub message: String,
}

impl Finding {
    pub(crate) fn hard(kind: &'static str, message: impl Into<String>) -> Self {
        Finding {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn advisory(kind: &'static str, message: impl Into<String>) -> Self {
        Finding {
            kind,
            message: message.into(),
        }
    }
}

/// Print findings in the shared format: `error [kind]: message` for hard
/// findings (to stderr) and `advisory [kind]: message` for advisory findings (to
/// stdout). `review`, `review --policy`, and `init-source` all print through this
/// so their findings read identically.
pub(crate) fn print_findings(hard: &[Finding], advisory: &[Finding]) {
    for f in hard {
        eprintln!("error [{}]: {}", f.kind, f.message);
    }
    for f in advisory {
        println!("advisory [{}]: {}", f.kind, f.message);
    }
}

/// The result of a review run.
pub struct ReviewResult {
    pub hard: Vec<Finding>,
    pub advisory: Vec<Finding>,
    /// Files rewritten by `--fix` (CLI-138), relative-or-absolute as displayed.
    /// Always empty unless `--fix` ran against a local target.
    pub fixed: Vec<String>,
}

/// `mind review <target> [--as <prefix>]`
///
/// Returns Ok(ReviewResult) where the hard and advisory findings are collected.
/// The caller decides whether to exit non-zero (any hard findings => non-zero).
///
/// `fix` enables the in-place token rewrite (CLI-138); it is honored only for a
/// local-path target and otherwise produces a hard refusal that changes nothing.
///
/// spec: CLI-130, CLI-131, CLI-132, CLI-133, CLI-135, CLI-136, CLI-137, CLI-138
pub fn review(
    paths: &Paths,
    target: &str,
    alias: Option<String>,
    fix: bool,
) -> Result<ReviewResult> {
    let (source_dir, _temp_guard, is_local) = resolve_target(paths, target, &alias)?;
    run_checks(paths, &source_dir, alias, fix, is_local)
}

/// `mind review --policy <path>` — validate a managed policy file.
///
/// Calls [`crate::policy::load_file`] at the given path. A parse error, unknown
/// key, or invariant violation surfaces as a hard finding and causes a non-zero
/// exit (reusing the same `ReviewResult`/exit machinery as source review so CLI
/// output and exit semantics are identical). On success, prints a clean "valid"
/// message and optionally advisory findings.
///
/// spec: POL-50
pub fn review_policy(path: &Path) -> crate::error::Result<ReviewResult> {
    let mut hard: Vec<Finding> = Vec::new();
    let mut advisory: Vec<Finding> = Vec::new();

    match crate::policy::load_file(path) {
        Err(e) => {
            hard.push(Finding::hard("invalid-policy", format!("{e}")));
        }
        Ok(policy) => {
            // Advisory: lock=true but allow is empty means everything is blocked.
            if policy.lock() && policy.allow().is_empty() {
                advisory.push(Finding::advisory(
                    "lock-without-allow",
                    "lock=true but [sources].allow is empty; every meld will be blocked",
                ));
            }
            // Advisory: auto_meld entries present without lock or pinned constraints
            // means org-provisioned sources float freely.
            if !policy.auto_meld().is_empty() && !policy.pinned() && !policy.lock() {
                advisory.push(Finding::advisory(
                    "unpinned-auto-meld",
                    "[[sources.auto_meld]] entries are present but pinned=false and lock=false; \
                     org-provisioned sources will track floating branches",
                ));
            }
        }
    }

    Ok(ReviewResult {
        hard,
        advisory,
        fixed: Vec::new(),
    })
}

/// Print the result of a `review --policy` run and exit non-zero on hard errors.
/// Mirrors the output format of `commands::review` so the two modes are consistent.
///
/// spec: POL-50
pub fn dispatch_policy(path: &Path) -> crate::error::Result<()> {
    let result = review_policy(path)?;

    print_findings(&result.hard, &result.advisory);

    if result.hard.is_empty() {
        if result.advisory.is_empty() {
            println!("review --policy: valid (no issues found)");
        } else {
            println!(
                "review --policy: valid ({} advisory finding(s))",
                result.advisory.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\nreview --policy: {} hard error(s), {} advisory finding(s)",
            result.hard.len(),
            result.advisory.len()
        );
        Err(MindError::ReviewFailed {
            hard: result.hard.len(),
        })
    }
}

/// Resolve `target` to a source directory. Returns the path and an optional
/// temp-dir guard that removes the cloned directory when dropped. The guard is
/// `Some` only for remote-spec targets.
///
/// Precedence: exact/suffix registry match > local path > remote spec.
/// spec: CLI-130
///
/// The trailing `bool` is `true` only for a local working-tree path: the one
/// target kind `--fix` (CLI-138) may rewrite. A registry selector resolves to
/// mind's managed clone and a repo spec to a discarded temp clone, so both are
/// `false`.
fn resolve_target(
    paths: &Paths,
    target: &str,
    alias: &Option<String>,
) -> Result<(PathBuf, Option<TempDirGuard>, bool)> {
    // Try registry match first (exact or suffix).
    // This covers both the melded-selector case and the "owner/repo" ambiguity.
    if let Some(dir) = try_registry_match(paths, target)? {
        return Ok((dir, None, false));
    }

    // Parse as a spec (local path or remote).
    let source = parse_spec(target)?;

    // Local path: the URL is the filesystem path; no clone needed.
    if source.host == "local" {
        let dir = PathBuf::from(&source.url);
        if !dir.is_dir() {
            return Err(MindError::Io {
                path: dir,
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"),
            });
        }
        let _ = alias; // alias is applied at check time, not here
        return Ok((dir, None, true));
    }

    // Remote spec: shallow-clone to a temp dir, register a drop guard.
    paths.ensure_layout()?;
    let tmp = paths.tmp_dir().join(format!(
        "review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    ));
    paths::mkdir_p(&tmp)?;
    let guard = TempDirGuard(tmp.clone());

    git::clone(&source.url, &tmp)?;
    Ok((tmp, Some(guard), false))
}

/// Look up `target` as a registry selector. Returns the clone dir if found.
fn try_registry_match(paths: &Paths, target: &str) -> Result<Option<PathBuf>> {
    let registry = Registry::load(paths)?;
    let matches: Vec<&Source> = registry
        .sources
        .iter()
        .filter(|s| source_matches(&s.name, target))
        .collect();

    match matches.as_slice() {
        [] => Ok(None),
        [only] => {
            let dir = only.clone_dir(paths);
            Ok(Some(dir))
        }
        many => Err(MindError::AmbiguousSource {
            query: target.to_string(),
            candidates: many.iter().map(|s| s.name.clone()).collect(),
        }),
    }
}

/// Run all checks against the source directory. Returns collected findings.
///
/// spec: CLI-131, CLI-132, CLI-133, CLI-135, CLI-136, CLI-137, CLI-138
fn run_checks(
    _paths: &Paths,
    source_dir: &Path,
    alias: Option<String>,
    fix: bool,
    is_local: bool,
) -> Result<ReviewResult> {
    let mut hard: Vec<Finding> = Vec::new();
    let mut advisory: Vec<Finding> = Vec::new();

    // --- Check 1: mind.toml parse + schema ---
    // A malformed or schema-violating mind.toml is a hard error (CLI-132).
    let mindfile = match MindToml::load(source_dir) {
        Ok(mf) => mf,
        Err(e) => {
            hard.push(Finding::hard(
                "toml-parse-error",
                format!("mind.toml error: {e}"),
            ));
            return Ok(ReviewResult {
                hard,
                advisory,
                fixed: Vec::new(),
            });
        }
    };

    // --- Check 2: conflicting [source] pin directive ---
    // Two pin directives is a hard error (CLI-132).
    if let Some(ref mf) = mindfile {
        let toml_path = source_dir.join("mind.toml");
        if let Err(e) = mf.source.pin_directive(&toml_path) {
            hard.push(Finding::hard("conflicting-pin", format!("{e}")));
            // Don't abort here; other checks may still be useful.
        }
    }

    // --- Check 6 (hoisted): declared hooks advisory ---
    // spec: HOOK-40, HOOK-58
    // Surface every declared hook (install + uninstall, required + optional) so
    // a consumer sees, before melding, that the source will ask to run arbitrary
    // code. This is placed before any early-returning scan checks so it fires on
    // every code path where the mind.toml parsed successfully and declares hooks,
    // including paths that encounter version-gate, unknown-kind, or other scan
    // errors later. It must NOT fire when mind.toml itself failed to parse
    // (Check 1's hard-error path returns before reaching here).
    //
    // resolved_hooks folds legacy [source].install (HOOK-50 back-compat) as the
    // first required install hook and appends [[hooks]] entries in declaration
    // order. An unknown event in [[hooks]] is already a hard mind.toml error
    // caught by scan later; here we propagate any unexpected Err rather than
    // double-reporting or silently swallowing it.
    if let Some(ref mf) = mindfile {
        let toml_path_for_hooks = source_dir.join("mind.toml");
        match mf.resolved_hooks(&toml_path_for_hooks) {
            Ok(hooks) => {
                for hook in &hooks {
                    let event_str = match hook.event {
                        crate::mindfile::HookEvent::Install => "install",
                        crate::mindfile::HookEvent::Uninstall => "uninstall",
                    };
                    let req_str = if hook.optional {
                        "optional"
                    } else {
                        "required"
                    };
                    advisory.push(Finding::advisory(
                        "install-hook",
                        format!(
                            "source declares a {} {} hook '{}': {}",
                            req_str,
                            event_str,
                            hook.label(),
                            hook.run,
                        ),
                    ));
                }
            }
            Err(e) => {
                // A bad event string is a schema error caught as a hard finding
                // by the catalog scan; propagate here to avoid dropping it.
                hard.push(Finding::hard(
                    "toml-parse-error",
                    format!("mind.toml error: {e}"),
                ));
                return Ok(ReviewResult {
                    hard,
                    advisory,
                    fixed: Vec::new(),
                });
            }
        }
    }

    // --- Check 3: catalog scan (version gate + unknown kind) ---
    // Build a synthetic source for scanning. Use scan_source_at so we can pass
    // the actual source_dir directly, bypassing the clone_dir() path resolution
    // that catalog::scan uses (which would require the dir to live at the
    // standard sources/<host>/<owner>/<repo> location).
    let source = build_source(source_dir, &mindfile, alias);

    // Scan. IncompatibleVersion and unknown-kind are hard errors (CLI-132).
    let mut items: Vec<CatalogItem> = Vec::new();
    match scan_source_at(source_dir, &source, &mut items) {
        Ok(()) => {}
        Err(MindError::IncompatibleVersion {
            ref source_name,
            ref required,
            ref running,
        }) => {
            hard.push(Finding::hard(
                "incompatible-version",
                format!(
                    "source '{}' requires mind >= {required}; this is mind {running}",
                    source_name
                ),
            ));
            return Ok(ReviewResult {
                hard,
                advisory,
                fixed: Vec::new(),
            });
        }
        Err(MindError::MindToml { ref msg, .. }) if msg.contains("unknown item kind") => {
            hard.push(Finding::hard(
                "unknown-kind",
                format!("mind.toml error: {msg}"),
            ));
            return Ok(ReviewResult {
                hard,
                advisory,
                fixed: Vec::new(),
            });
        }
        Err(e) => {
            hard.push(Finding::hard("scan-error", format!("scan error: {e}")));
            return Ok(ReviewResult {
                hard,
                advisory,
                fixed: Vec::new(),
            });
        }
    };

    // --- Check 4: missing descriptions (advisory) ---
    // CLI-132: missing description is advisory only.
    for item in &items {
        if item.description.is_none() {
            advisory.push(Finding::advisory(
                "missing-description",
                format!("{}: no description in frontmatter or mind.toml", item.key()),
            ));
        }
    }

    // --- Check 8: per-item install/uninstall hooks (advisory) ---
    // spec: HOOK-85
    // Surface each item that declares an install or uninstall hook so a consumer
    // sees, before installing, that the item will run code on the host (the
    // item-level counterpart of the source-hook disclosure, HOOK-40/58).
    for item in &items {
        for (event, cmd) in [
            ("install", item.install.as_deref()),
            ("uninstall", item.uninstall.as_deref()),
        ] {
            if let Some(cmd) = cmd {
                advisory.push(Finding::advisory(
                    "item-hook",
                    format!("{}: declares an {event} hook '{cmd}'", item.key()),
                ));
            }
        }
    }

    // --- Check 5: {{ns:}} token resolution (hard error) ---
    // An unresolved {{ns:}} token would be a BadReference at install time.
    // spec: CLI-132
    let source_name = source.name.clone();
    let siblings = siblings_of_source(&items, &source_name);
    let prefix = source
        .alias
        .clone()
        .or_else(|| mindfile.as_ref().and_then(|m| m.source.prefix.clone()));

    for item in &items {
        for file in item_files(item) {
            let content = match std::fs::read_to_string(&file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Err(bad_ref) = crate::namespace::expand(&content, &prefix, &siblings) {
                hard.push(Finding::hard(
                    "bad-reference",
                    format!(
                        "{}: {{{{ns:{}}}}} does not resolve to any sibling in this source",
                        item.key(),
                        bad_ref
                    ),
                ));
            }
        }
    }

    // --- Check 7: unguarded prose references (advisory, only when prefix in effect) ---
    // CLI-132: advisory; CLI-133: only when a prefix applies.
    if prefix.is_some() {
        for item in &items {
            let mut refs: Vec<String> = Vec::new();
            for file in item_files(item) {
                let Ok(content) = std::fs::read_to_string(&file) else {
                    continue;
                };
                for r in crate::namespace::unguarded_refs(&content, &siblings) {
                    if r != item.name && !refs.contains(&r) {
                        refs.push(r);
                    }
                }
            }
            if !refs.is_empty() {
                advisory.push(Finding::advisory(
                    "unguarded-reference",
                    format!(
                        "{}: references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                        item.key(),
                        refs.join(", ")
                    ),
                ));
            }
        }
    }

    // Per-source path-token resolver: every item's (kind, name, entrypoint).
    // The store_root is a placeholder; review only cares whether a token
    // resolves (Ok) or not (Err), never the concrete install path.
    let path_siblings: Vec<crate::namespace::PathSibling> =
        items.iter().map(CatalogItem::as_path_sibling).collect();
    let store_root = std::path::Path::new("/");

    for item in &items {
        let ctx = crate::namespace::PathCtx {
            store_root,
            home: None,
            prefix: &prefix,
            self_kind: item.kind,
            self_name: &item.name,
            siblings: &path_siblings,
        };
        let mut bare_tools: Vec<String> = Vec::new();
        for file in item_files(item) {
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            // A non-markdown item file (a script, data) is entirely code: any
            // `{{ns:}}` in it is misplaced (NS-24).
            let is_md = file.extension().and_then(|e| e.to_str()) == Some("md");
            // Check 8: path-reference tokens that do not resolve (hard, CLI-135).
            if let Err(token) = crate::namespace::expand_paths(&content, &ctx) {
                hard.push(Finding::hard(
                    "bad-reference",
                    format!(
                        "{}: {token} does not resolve to any sibling in this source",
                        item.key()
                    ),
                ));
            }
            // Check 9: hardcoded install paths that should be tokens (advisory,
            // CLI-136). The wording reflects what the path resolves to (CLI-145).
            for hp in crate::namespace::detect_hardcoded_paths(&content, &ctx) {
                let suggestion = match &hp.suggestion {
                    Some(tok) => format!("; use {tok}"),
                    None => String::new(),
                };
                let msg = match hp.kind {
                    crate::namespace::HardcodedKind::OwnResource => format!(
                        "{}: hardcodes its own resource path '{}'; this works but assumes every install lands at that exact agent-home path, so it breaks under a prefix or a second home{}",
                        item.key(),
                        hp.matched,
                        suggestion
                    ),
                    crate::namespace::HardcodedKind::SharedTool => format!(
                        "{}: hardcodes a shared tool path '{}'; a tool is store-only and never linked into an agent home, so this will not resolve{}",
                        item.key(),
                        hp.matched,
                        suggestion
                    ),
                    crate::namespace::HardcodedKind::OtherItem => format!(
                        "{}: hardcoded install path '{}'; a literal mind install path is fragile (a path token tracks it, or install the resource to a fixed location via an install hook){}",
                        item.key(),
                        hp.matched,
                        suggestion
                    ),
                };
                advisory.push(Finding::advisory("hardcoded-path", msg));
            }
            // Check 10: sibling tools named in prose without a token (advisory, CLI-137).
            for t in crate::namespace::bare_tool_refs(&content, &path_siblings) {
                if t != item.name && !bare_tools.contains(&t) {
                    bare_tools.push(t);
                }
            }
            // Check 11: misplaced {{ns:}} tokens (CLI-139). Code/path -> advisory;
            // the frontmatter `name:` field -> hard (an item must not namespace
            // its own name).
            for r in crate::namespace::scan_ns_refs(&content) {
                // In a non-markdown file the whole text is code, so treat any
                // token as code-block context.
                let context = if is_md {
                    r.context
                } else {
                    crate::namespace::NsContext::CodeBlock
                };
                let where_ = match context {
                    crate::namespace::NsContext::Prose => continue,
                    crate::namespace::NsContext::CodeBlock if !is_md => "a non-markdown file",
                    crate::namespace::NsContext::CodeBlock => "a code block",
                    crate::namespace::NsContext::CodeSpan => "a code span",
                    crate::namespace::NsContext::Path => "a path",
                    crate::namespace::NsContext::FrontmatterName => "the frontmatter `name:` field",
                };
                let msg = format!(
                    "{}: {{{{ns:{}}}}} in {where_}; a name token belongs in prose (code/paths use {{{{tools:}}}}/{{{{self}}}}/{{{{path:}}}})",
                    item.key(),
                    r.name
                );
                if context == crate::namespace::NsContext::FrontmatterName {
                    hard.push(Finding::hard("misplaced-reference", msg));
                } else {
                    advisory.push(Finding::advisory("misplaced-reference", msg));
                }
            }
        }
        if !bare_tools.is_empty() {
            advisory.push(Finding::advisory(
                "bare-tool-reference",
                format!(
                    "{}: names tool item(s) in prose: {}; a tool item is reached by a token ({{{{tools:name}}}}), or install the helper to a known location via an install hook and call it there",
                    item.key(),
                    bare_tools.join(", ")
                ),
            ));
        }
    }

    // Check 12: helper scripts duplicated across items (advisory, CLI-144).
    advisory.extend(duplicate_tooling_findings(&items));

    // Fix (CLI-138): rewrite the local working copy in place. Local-path target
    // only; a registry selector or repo spec is refused with nothing changed.
    let mut fixed: Vec<String> = Vec::new();
    if fix {
        if !is_local {
            hard.push(Finding::hard(
                "fix-not-local",
                "--fix only rewrites a local-path source; a melded selector or repo spec is \
                 not the author's working tree, so nothing was changed",
            ));
        } else {
            for item in &items {
                let ctx = crate::namespace::PathCtx {
                    store_root,
                    home: None,
                    prefix: &prefix,
                    self_kind: item.kind,
                    self_name: &item.name,
                    siblings: &path_siblings,
                };
                for file in item_files(item) {
                    let Ok(content) = std::fs::read_to_string(&file) else {
                        continue;
                    };
                    let is_md = file.extension().and_then(|e| e.to_str()) == Some("md");
                    // Hardcoded install paths -> tokens (any file).
                    let (s1, n1) = crate::namespace::rewrite_hardcoded_paths(&content, &ctx);
                    // Un-wrap misplaced {{ns:}}; a non-markdown file is all code,
                    // so every token is misplaced there.
                    let (s2, n2) = crate::namespace::unwrap_misplaced(&s1, !is_md);
                    // Templatize bare prose refs -> {{ns:}}, markdown only (a
                    // script is all code; wrapping a keyword there is the bug
                    // this whole path exists to avoid).
                    let (s3, n3) = if is_md {
                        crate::namespace::templatize(&s2, &siblings)
                    } else {
                        (s2, 0)
                    };
                    if n1 + n2 + n3 > 0 {
                        std::fs::write(&file, s3).map_err(|e| MindError::io(&file, e))?;
                        fixed.push(file.display().to_string());
                    }
                }
            }
        }
    }

    Ok(ReviewResult {
        hard,
        advisory,
        fixed,
    })
}

/// Build a synthetic `Source` for the directory being reviewed.
fn build_source(source_dir: &Path, mindfile: &Option<MindToml>, alias: Option<String>) -> Source {
    // Derive a source-like identity from the path.
    let url = source_dir.to_string_lossy().into_owned();
    let mut comps = url.trim_end_matches('/').rsplit('/');
    let repo = comps
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("repo")
        .to_string();
    let owner = comps
        .next()
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("local")
        .to_string();

    let description = mindfile.as_ref().and_then(|m| m.source.description.clone());

    Source {
        name: format!("local/{owner}/{repo}"),
        url: url.clone(),
        host: "local".to_string(),
        owner,
        repo,
        commit: None,
        description,
        alias,
        pin: crate::source::Pin::default(),
        roots: None,
        install_hooks: Vec::new(),
        install_hook: None,
        install_hook_commit: None,
    }
}

/// The set of bare item names for the given source, for reference validation.
fn siblings_of_source(items: &[CatalogItem], source: &str) -> HashSet<String> {
    items
        .iter()
        .filter(|it| it.source == source)
        .map(|it| it.name.clone())
        .collect()
}

/// Detect helper files duplicated byte-for-byte across two or more items, which
/// COULD live once under a shared `tools/<name>/` and be referenced by token, or
/// stay siloed per item (both valid; CLI-144 / INIT-7). Only non-markdown files
/// are considered: markdown is
/// prose (the anchor `SKILL.md`, docs), while scripts and data are the helpers a
/// `tool` exists to share. Empty files are ignored. Returns one advisory
/// `duplicate-tooling` finding per duplicated file, deterministically ordered.
///
/// Both callers (`review`, `init_source`) scan a single source, so a match is a
/// genuine within-source duplicate, not a coincidence across unrelated repos.
pub(crate) fn duplicate_tooling_findings(items: &[CatalogItem]) -> Vec<Finding> {
    use std::collections::{BTreeMap, BTreeSet};
    // content hash -> (file basename, the item keys whose dir holds that content)
    let mut groups: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    for item in items {
        for file in item_files(item) {
            if file.extension().and_then(|e| e.to_str()) == Some("md") {
                continue;
            }
            // Skip empty files: a shared zero-byte placeholder is not tooling.
            match std::fs::metadata(&file) {
                Ok(m) if m.len() == 0 => continue,
                Ok(_) => {}
                Err(_) => continue,
            }
            let Ok(hash) = crate::hash::hash_path(&file) else {
                continue;
            };
            let base = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let entry = groups
                .entry(hash)
                .or_insert_with(|| (base, BTreeSet::new()));
            entry.1.insert(item.key());
        }
    }
    groups
        .into_values()
        .filter(|(_, owners)| owners.len() >= 2)
        .map(|(base, owners)| {
            Finding::advisory(
                "duplicate-tooling",
                format!(
                    "{base} is byte-identical across {}; it could be shared once as a tool (tools/<name>/) referenced by a token ({{{{tools:name}}}} or {{{{path:}}}}), or kept as-is if each item should bundle its own helper (both are valid)",
                    owners.into_iter().collect::<Vec<_>>().join(", ")
                ),
            )
        })
        .collect()
}

/// All text files for an item: every file under a skill dir, or the single
/// agent/rule file.
pub(crate) fn item_files(item: &CatalogItem) -> Vec<PathBuf> {
    if item.path.is_dir() {
        let mut files = Vec::new();
        collect_files(&item.path, &mut files);
        files.sort();
        files
    } else {
        vec![item.path.clone()]
    }
}

/// Recursively collect every file under `dir`.
pub(crate) fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

/// A RAII guard that removes a temp directory on drop.
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::source::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};

    static UNIT_COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let n = UNIT_COUNTER.fetch_add(1, Ordering::SeqCst);
            let p =
                std::env::temp_dir().join(format!("mind-review-unit-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn paths_for(base: &Path) -> Paths {
        Paths {
            mind_home: base.to_path_buf(),
            claude_home: base.join("claude"),
        }
    }

    /// Build a registry Source pointing at `clone_dir` and a Paths that sees it.
    fn make_source(name: &str, clone_dir: &Path) -> Source {
        Source {
            name: name.to_string(),
            url: clone_dir.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: name.rsplit('/').next().unwrap_or(name).to_string(),
            commit: None,
            description: None,
            alias: None,
            pin: Pin::default(),
            roots: None,
            install_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
        }
    }

    // --- target resolution precedence (CLI-130) ---

    /// When a bare `owner/repo`-style string matches a registry entry, it must
    /// resolve via the registry (not be re-parsed as a spec).
    /// spec: CLI-130
    #[test]
    fn registry_match_beats_spec_for_ambiguous_target() {
        let tmp = TmpDir::new();
        let base = tmp.path();

        // Build a minimal registry with one source whose name ends in "agents".
        let source_dir = base.join("sources/local/test/agents");
        write_file(
            &source_dir.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );

        // Write a sources.json pointing at the source dir.
        let registry = crate::source::Registry {
            sources: vec![make_source("local/test/agents", &source_dir)],
        };
        let paths = paths_for(base);
        std::fs::create_dir_all(base.join("sources")).unwrap();
        registry.save(&paths).unwrap();

        // "agents" is a suffix match on "local/test/agents" in the registry.
        let result = try_registry_match(&paths, "agents").unwrap();
        assert!(
            result.is_some(),
            "suffix match in registry must return the clone dir"
        );
        let dir = result.unwrap();
        assert!(
            dir.ends_with("agents"),
            "should resolve to the registered clone dir: {dir:?}"
        );
    }

    /// A target that matches no registry entry is returned as None, so the
    /// caller can fall through to spec/path parsing.
    /// spec: CLI-130
    #[test]
    fn unknown_target_returns_none_from_registry_match() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let paths = paths_for(base);

        let result = try_registry_match(&paths, "owner/nonexistent").unwrap();
        assert!(result.is_none(), "no match should give None");
    }

    // --- hard vs advisory classification (CLI-132) ---

    /// A source with all valid items and descriptions has no findings at all.
    /// spec: CLI-130, CLI-131
    #[test]
    fn clean_source_has_no_findings() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("skills/review/SKILL.md"),
            "---\ndescription: Review the diff\n---\n# review\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "expected no hard findings: {:?}",
            result.hard
        );
        assert!(
            result.advisory.is_empty(),
            "expected no advisory findings: {:?}",
            result.advisory
        );
    }

    /// Missing description is advisory, not hard.
    /// spec: CLI-132
    #[test]
    fn missing_description_is_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/dev.md"),
            "# dev agent\nno frontmatter here\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "missing description must not be hard"
        );
        assert!(
            result
                .advisory
                .iter()
                .any(|f| f.kind == "missing-description"),
            "expected missing-description advisory: {:?}",
            result.advisory
        );
    }

    /// A malformed mind.toml (TOML parse error) is a hard error.
    /// spec: CLI-132
    #[test]
    fn malformed_toml_is_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("mind.toml"), "[[[[bad toml").unwrap();
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            !result.hard.is_empty(),
            "malformed TOML must produce a hard finding"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "toml-parse-error"),
            "expected toml-parse-error: {:?}",
            result.hard
        );
    }

    /// An unknown item kind in [[items]] is a hard error.
    /// spec: CLI-132
    #[test]
    fn unknown_item_kind_is_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x.md\"\n",
        )
        .unwrap();
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            !result.hard.is_empty(),
            "unknown kind must produce a hard finding"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "unknown-kind"),
            "expected unknown-kind: {:?}",
            result.hard
        );
    }

    /// A conflicting [source] pin directive is a hard error.
    /// spec: CLI-132
    #[test]
    fn conflicting_pin_is_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\nfollow-branch = \"main\"\npin-tag = \"v1.0\"\n",
        )
        .unwrap();
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            !result.hard.is_empty(),
            "conflicting pin must produce a hard finding"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "conflicting-pin"),
            "expected conflicting-pin: {:?}",
            result.hard
        );
    }

    /// An unresolved {{ns:}} token is a hard error.
    /// spec: CLI-132
    #[test]
    fn bad_ns_token_is_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            !result.hard.is_empty(),
            "unresolved ns token must produce a hard finding"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "bad-reference"),
            "expected bad-reference: {:?}",
            result.hard
        );
    }

    /// Unguarded prose references under a prefix are advisory, not hard.
    /// spec: CLI-132, CLI-133
    #[test]
    fn unguarded_ref_under_prefix_is_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );
        let paths = paths_for(base);

        // With --as jk: a prefix is in effect, so the unguarded ref is flagged.
        let result = run_checks(&paths, &source_dir, Some("jk".to_string()), false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "unguarded ref must not be hard: {:?}",
            result.hard
        );
        assert!(
            result
                .advisory
                .iter()
                .any(|f| f.kind == "unguarded-reference"),
            "expected unguarded-reference advisory: {:?}",
            result.advisory
        );
    }

    /// A source whose mind.toml declares [source].install produces an advisory
    /// finding with kind "install-hook" containing the declared command.
    /// spec: HOOK-40
    #[test]
    fn declared_install_hook_is_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ninstall = \"make build && make install\"\n",
        )
        .unwrap();
        // Add a valid item so the source is otherwise clean.
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "declared install hook must not be a hard finding: {:?}",
            result.hard
        );
        let hook_findings: Vec<&Finding> = result
            .advisory
            .iter()
            .filter(|f| f.kind == "install-hook")
            .collect();
        assert_eq!(
            hook_findings.len(),
            1,
            "expected exactly one install-hook advisory: {:?}",
            result.advisory
        );
        assert!(
            hook_findings[0]
                .message
                .contains("make build && make install"),
            "advisory message must include the declared command: {}",
            hook_findings[0].message
        );
    }

    /// A source that BOTH declares [source].install AND triggers a hard scan
    /// error (unknown item kind in [[items]]) must still emit the install-hook
    /// advisory. Before the fix, the early return from Check 3 discarded the
    /// advisory. After the fix, the advisory is pushed before Check 3 runs.
    ///
    /// This test is the regression guard for the HOOK-40 reachability bug: it
    /// must FAIL against code where Check 6 is placed after Check 3, and PASS
    /// after the hoist.
    /// spec: HOOK-40
    #[test]
    fn install_hook_advisory_survives_scan_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        // Authoritative mind.toml: declares an install hook AND an unknown item
        // kind. The unknown-kind arm triggers an early return from the scan check.
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ninstall = \"make tooling\"\n\
             [[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x.md\"\n",
        )
        .unwrap();
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();

        // The scan must have produced the unknown-kind hard finding.
        assert!(
            result.hard.iter().any(|f| f.kind == "unknown-kind"),
            "expected unknown-kind hard finding: {:?}",
            result.hard
        );

        // The install-hook advisory must still be present despite the early return.
        let hook_findings: Vec<&Finding> = result
            .advisory
            .iter()
            .filter(|f| f.kind == "install-hook")
            .collect();
        assert_eq!(
            hook_findings.len(),
            1,
            "install-hook advisory must survive an early-returning scan error (HOOK-40 reachability bug): {:?}",
            result.advisory
        );
        assert!(
            hook_findings[0].message.contains("make tooling"),
            "advisory must include the declared command: {}",
            hook_findings[0].message
        );
    }

    /// A source with no [source].install declared produces no install-hook
    /// advisory. Confirms the check is absent, not spurious.
    /// spec: HOOK-40
    #[test]
    fn no_install_hook_produces_no_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ndescription = \"tools\"\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.advisory.iter().all(|f| f.kind != "install-hook"),
            "no install hook declared => no install-hook advisory: {:?}",
            result.advisory
        );
    }

    /// A source whose mind.toml declares multiple [[hooks]] (an install hook and
    /// an optional uninstall hook) produces one advisory finding per hook,
    /// each mentioning the hook's label, event, optional/required status, and
    /// command.
    /// spec: HOOK-40, HOOK-58
    #[test]
    fn multi_hook_declarations_each_produce_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\n\
             [[hooks]]\n\
             run = \"npm install\"\n\
             name = \"Install deps\"\n\
             event = \"install\"\n\
             [[hooks]]\n\
             run = \"npm run cleanup\"\n\
             optional = true\n\
             event = \"uninstall\"\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "multi-hook source must not produce hard findings: {:?}",
            result.hard
        );

        let hook_findings: Vec<&Finding> = result
            .advisory
            .iter()
            .filter(|f| f.kind == "install-hook")
            .collect();
        assert_eq!(
            hook_findings.len(),
            2,
            "expected one advisory per hook: {:?}",
            result.advisory
        );

        // First hook: required install, named "Install deps", runs "npm install".
        let f0 = &hook_findings[0];
        assert!(
            f0.message.contains("required"),
            "first hook must be required: {}",
            f0.message
        );
        assert!(
            f0.message.contains("install"),
            "first hook must be an install event: {}",
            f0.message
        );
        assert!(
            f0.message.contains("Install deps"),
            "first hook label must appear: {}",
            f0.message
        );
        assert!(
            f0.message.contains("npm install"),
            "first hook command must appear: {}",
            f0.message
        );

        // Second hook: optional uninstall, label falls back to command.
        let f1 = &hook_findings[1];
        assert!(
            f1.message.contains("optional"),
            "second hook must be optional: {}",
            f1.message
        );
        assert!(
            f1.message.contains("uninstall"),
            "second hook must be an uninstall event: {}",
            f1.message
        );
        assert!(
            f1.message.contains("npm run cleanup"),
            "second hook command must appear: {}",
            f1.message
        );
    }

    /// The legacy [source].install field still surfaces as a required install
    /// hook advisory (HOOK-40 back-compat), now described via resolved_hooks.
    /// The advisory message must include the command.
    /// spec: HOOK-40, HOOK-58
    #[test]
    fn legacy_install_field_still_surfaces_as_required_install_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ninstall = \"make setup\"\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "legacy install hook must not be hard: {:?}",
            result.hard
        );
        let hook_findings: Vec<&Finding> = result
            .advisory
            .iter()
            .filter(|f| f.kind == "install-hook")
            .collect();
        assert_eq!(
            hook_findings.len(),
            1,
            "expected exactly one advisory for legacy install: {:?}",
            result.advisory
        );
        let msg = &hook_findings[0].message;
        assert!(
            msg.contains("required"),
            "legacy install must be reported as required: {msg}"
        );
        assert!(
            msg.contains("install"),
            "legacy install must be reported as install event: {msg}"
        );
        assert!(
            msg.contains("make setup"),
            "legacy install advisory must include the command: {msg}"
        );
    }

    /// When mind.toml declares both a legacy [source].install AND [[hooks]]
    /// entries, resolved_hooks folds them together and review emits one finding
    /// per resolved hook.
    /// spec: HOOK-58
    #[test]
    fn legacy_and_hooks_table_both_surface() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ninstall = \"make legacy\"\n\
             [[hooks]]\nrun = \"cleanup.sh\"\nevent = \"uninstall\"\noptional = true\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "no hard findings expected: {:?}",
            result.hard
        );
        let hook_findings: Vec<&Finding> = result
            .advisory
            .iter()
            .filter(|f| f.kind == "install-hook")
            .collect();
        assert_eq!(
            hook_findings.len(),
            2,
            "legacy + hooks table must each produce an advisory: {:?}",
            result.advisory
        );
        // First: legacy required install.
        assert!(hook_findings[0].message.contains("make legacy"));
        assert!(hook_findings[0].message.contains("required"));
        // Second: optional uninstall.
        assert!(hook_findings[1].message.contains("cleanup.sh"));
        assert!(hook_findings[1].message.contains("optional"));
        assert!(hook_findings[1].message.contains("uninstall"));
    }

    /// A source with no hooks (no [source].install, no [[hooks]]) produces no
    /// hook advisory finding.
    /// spec: HOOK-58
    #[test]
    fn no_hooks_at_all_produces_no_hook_advisory() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\ndescription = \"clean source\"\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/tool.md"),
            "---\ndescription: tool agent\n---\n# tool\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.advisory.iter().all(|f| f.kind != "install-hook"),
            "no hooks declared => no install-hook advisory: {:?}",
            result.advisory
        );
    }

    /// Without a prefix, unguarded refs are not reported at all.
    /// spec: CLI-133
    #[test]
    fn unguarded_ref_without_prefix_not_reported() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );
        let paths = paths_for(base);

        // No alias: no prefix, unguarded refs irrelevant.
        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(result.hard.is_empty());
        assert!(
            result
                .advisory
                .iter()
                .all(|f| f.kind != "unguarded-reference"),
            "unguarded ref should not be reported without a prefix: {:?}",
            result.advisory
        );
    }

    /// A source with a [source].prefix that is not overridden by --as also
    /// triggers unguarded-reference detection.
    /// spec: CLI-131, CLI-133
    #[test]
    fn source_prefix_also_triggers_unguarded_check() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("mind.toml"), "[source]\nprefix = \"ag\"\n").unwrap();
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to dev.\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );
        let paths = paths_for(base);

        // No consumer alias: the source's own prefix should trigger the check.
        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "should be hard-clean: {:?}",
            result.hard
        );
        assert!(
            result
                .advisory
                .iter()
                .any(|f| f.kind == "unguarded-reference"),
            "source prefix should trigger unguarded-reference check: {:?}",
            result.advisory
        );
    }

    /// A `min-mind-version` the running binary cannot satisfy is a hard error
    /// classified as `incompatible-version` (not a generic scan error). The
    /// existing tests never exercise the IncompatibleVersion scan arm.
    /// spec: CLI-131, CLI-132
    #[test]
    fn incompatible_min_version_is_hard_error() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("mind.toml"),
            "[source]\nmin-mind-version = \"99.0\"\n",
        )
        .unwrap();
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.iter().any(|f| f.kind == "incompatible-version"),
            "expected incompatible-version hard finding: {:?}",
            result.hard
        );
    }

    /// Two distinct bad `{{ns:}}` tokens in two items must BOTH be reported as
    /// hard findings, not short-circuit after the first. CLI-132 counts every
    /// hard finding so the printer can report the true total. This guards the
    /// no-`return`-after-bad-reference behavior of Check 5.
    /// spec: CLI-131, CLI-132
    #[test]
    fn multiple_bad_ns_tokens_all_reported() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
        );
        write_file(
            &source_dir.join("agents/boss.md"),
            "---\ndescription: boss\n---\nDefer to {{ns:alsonope}}.\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        let bad: Vec<&Finding> = result
            .hard
            .iter()
            .filter(|f| f.kind == "bad-reference")
            .collect();
        assert_eq!(
            bad.len(),
            2,
            "both bad-reference findings must be present: {:?}",
            result.hard
        );
    }

    /// A bad `{{ns:}}` token AND an unknown item kind are different hard checks;
    /// here the unknown kind aborts the scan, so the report is the unknown-kind
    /// finding. This documents that an unknown kind is a scan-level hard error
    /// that prevents per-item reference checks (which need a scanned catalog).
    /// spec: CLI-132
    #[test]
    fn unknown_kind_blocks_before_reference_checks() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        // Authoritative mind.toml with a bad kind: scan fails before items exist.
        std::fs::write(
            source_dir.join("mind.toml"),
            "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x.md\"\n",
        )
        .unwrap();
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
        );
        let paths = paths_for(base);

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.iter().any(|f| f.kind == "unknown-kind"),
            "expected unknown-kind to surface: {:?}",
            result.hard
        );
        // The scan aborted, so no per-item reference checks ran.
        assert!(
            result.hard.iter().all(|f| f.kind != "bad-reference"),
            "reference checks must not run once the scan failed: {:?}",
            result.hard
        );
    }

    /// `--as <prefix>` overrides the source's own `[source].prefix` for token
    /// expansion: a token that resolves to a *prefixed* sibling stays clean.
    /// Confirms the consumer alias flows into effective-name resolution.
    /// spec: CLI-133
    #[test]
    fn alias_flows_into_token_resolution() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let source_dir = base.join("src");
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to {{ns:dev}}.\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );
        let paths = paths_for(base);

        // dev is a real sibling, so {{ns:dev}} resolves under any prefix.
        let result = run_checks(&paths, &source_dir, Some("jk".to_string()), false, true).unwrap();
        assert!(
            result.hard.iter().all(|f| f.kind != "bad-reference"),
            "valid token must resolve under --as prefix: {:?}",
            result.hard
        );
    }

    // --- registry resolution edge cases (CLI-130) ---

    /// A selector that matches two registered sources is an AmbiguousSource
    /// error, not a panic or a silent pick. `try_registry_match` must surface
    /// every candidate.
    /// spec: CLI-130
    #[test]
    fn ambiguous_selector_errors_with_candidates() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let paths = paths_for(base);
        std::fs::create_dir_all(base.join("sources")).unwrap();

        // Two sources whose names both end in "agents".
        let d1 = base.join("sources/local/alice/agents");
        let d2 = base.join("sources/local/bob/agents");
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        let registry = crate::source::Registry {
            sources: vec![
                make_source("local/alice/agents", &d1),
                make_source("local/bob/agents", &d2),
            ],
        };
        registry.save(&paths).unwrap();

        let err = try_registry_match(&paths, "agents").unwrap_err();
        match err {
            MindError::AmbiguousSource { query, candidates } => {
                assert_eq!(query, "agents");
                assert_eq!(
                    candidates.len(),
                    2,
                    "both candidates listed: {candidates:?}"
                );
            }
            other => panic!("expected AmbiguousSource, got {other:?}"),
        }
    }

    // --- repo-spec clone + temp cleanup (CLI-130) ---
    //
    // The remote-spec branch of `resolve_target` is not reachable through the
    // public `review()` entry point without a network remote (parse_spec maps
    // every offline-cloneable target to host="local", which skips the clone).
    // These tests drive the exact clone+guard+cleanup sequence that branch runs,
    // against a real local bare repo, so the no-disk-change guarantee (CLI-130)
    // is exercised end to end and a neutered guard would fail them.

    /// Build a bare git repo at `base/remote.git` containing one clean skill,
    /// returning its path. `git clone <path>` works fully offline.
    fn make_bare_remote(base: &Path) -> PathBuf {
        let work = base.join("remote-work");
        write_file(
            &work.join("skills/review/SKILL.md"),
            "---\ndescription: Review the diff\n---\n# review\n",
        );
        run_git(&work, &["-c", "init.defaultBranch=main", "init", "-q"]);
        run_git(&work, &["config", "user.email", "t@t"]);
        run_git(&work, &["config", "user.name", "t"]);
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-qm", "initial"]);

        let bare = base.join("remote.git");
        run_git(
            base,
            &[
                "clone",
                "--bare",
                "-q",
                &work.to_string_lossy(),
                &bare.to_string_lossy(),
            ],
        );
        bare
    }

    fn run_git(dir: &Path, args: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    /// Replicates resolve_target's remote branch: mkdir a temp dir under
    /// MIND_HOME/.tmp, install the drop guard, clone the bare repo into it, run
    /// the checks, then drop the guard. After the guard drops, the temp area
    /// must be empty (CLI-130: clone removed after the check). Mirrors the
    /// production sequence at review.rs resolve_target lines 96-109.
    fn review_via_clone(paths: &Paths, bare: &Path, alias: Option<String>) -> ReviewResult {
        paths.ensure_layout().unwrap();
        let tmp = paths.tmp_dir().join(format!(
            "review-{}-{}",
            std::process::id(),
            UNIT_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        paths::mkdir_p(&tmp).unwrap();
        let guard = TempDirGuard(tmp.clone());
        git::clone(&bare.to_string_lossy(), &tmp).unwrap();
        assert!(tmp.is_dir(), "clone target should exist mid-review");
        let result = run_checks(paths, &tmp, alias, false, false).unwrap();
        drop(guard);
        assert!(
            !tmp.exists(),
            "TempDirGuard must remove the clone on drop: {tmp:?}"
        );
        result
    }

    /// A successful review of a freshly cloned bare repo leaves no temp dir
    /// behind and the MIND_HOME/.tmp scratch area is empty afterward.
    /// spec: CLI-130
    #[test]
    fn clone_review_success_leaves_no_temp() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        let bare = make_bare_remote(base);
        let paths = paths_for(base);

        let result = review_via_clone(&paths, &bare, None);
        assert!(result.hard.is_empty(), "clean clone: {:?}", result.hard);
        assert_no_review_temp(&paths);
    }

    /// Even when the cloned repo has a HARD finding, the temp clone is still
    /// removed. The guard drops at the end of the review scope regardless of the
    /// findings. spec: CLI-130, CLI-132
    #[test]
    fn clone_review_with_hard_finding_still_cleans_up() {
        let tmp = TmpDir::new();
        let base = tmp.path();
        // A bare repo whose only item carries an unresolved {{ns:}} token.
        let work = base.join("remote-work");
        write_file(
            &work.join("agents/lead.md"),
            "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
        );
        run_git(&work, &["-c", "init.defaultBranch=main", "init", "-q"]);
        run_git(&work, &["config", "user.email", "t@t"]);
        run_git(&work, &["config", "user.name", "t"]);
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-qm", "bad"]);
        let bare = base.join("remote.git");
        run_git(
            base,
            &[
                "clone",
                "--bare",
                "-q",
                &work.to_string_lossy(),
                &bare.to_string_lossy(),
            ],
        );
        let paths = paths_for(base);

        let result = review_via_clone(&paths, &bare, None);
        assert!(
            result.hard.iter().any(|f| f.kind == "bad-reference"),
            "expected the hard finding from the clone: {:?}",
            result.hard
        );
        assert_no_review_temp(&paths);
    }

    // --- review --policy tests (POL-50) ---

    /// A well-formed policy file produces no hard findings and exits successfully.
    /// spec: POL-50
    #[test]
    fn policy_review_valid_file_passes() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(
            &policy_path,
            "[sources]\nallow = [\"github.com/acme/*\"]\nlock = true\n\
             [[sources.auto_meld]]\nrepo = \"acme/baseline\"\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            result.hard.is_empty(),
            "valid policy must have no hard findings: {:?}",
            result.hard
        );
    }

    /// A policy with an unknown key (POL-5) is reported as a hard finding.
    /// spec: POL-50
    #[test]
    fn policy_review_unknown_key_is_hard() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        // "allowed" is not a valid key; "allow" is.
        std::fs::write(
            &policy_path,
            "[sources]\nallowed = [\"github.com/acme/*\"]\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            !result.hard.is_empty(),
            "unknown key must produce a hard finding"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "invalid-policy"),
            "expected invalid-policy finding: {:?}",
            result.hard
        );
    }

    /// A policy with pinned=true and an unpinned auto_meld entry (POL-21) is
    /// reported as a hard finding by review.
    /// spec: POL-50
    #[test]
    fn policy_review_unpinned_entry_with_pinned_flag_is_hard() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        // auto_meld entry uses default branch (no tag/ref), but pinned=true.
        std::fs::write(
            &policy_path,
            "[sources]\npinned = true\n\
             [[sources.auto_meld]]\nrepo = \"github.com/acme/baseline\"\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            !result.hard.is_empty(),
            "pinned=true with unpinned auto_meld must be a hard finding (POL-21)"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "invalid-policy"),
            "expected invalid-policy finding: {:?}",
            result.hard
        );
    }

    /// A policy with lock=true and an auto_meld entry outside allow (POL-31) is
    /// reported as a hard finding by review.
    /// spec: POL-50
    #[test]
    fn policy_review_auto_meld_outside_allow_with_lock_is_hard() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        // auto_meld repo "github.com/other/x" does not match allow pattern.
        std::fs::write(
            &policy_path,
            "[sources]\nlock = true\nallow = [\"github.com/acme/*\"]\n\
             [[sources.auto_meld]]\nrepo = \"github.com/other/x\"\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            !result.hard.is_empty(),
            "lock=true with auto_meld outside allow must be a hard finding (POL-31)"
        );
        assert!(
            result.hard.iter().any(|f| f.kind == "invalid-policy"),
            "expected invalid-policy finding: {:?}",
            result.hard
        );
    }

    /// A malformed (un-parseable) TOML policy is a hard `invalid-policy`
    /// finding, exactly like an unknown key. Drives `review_policy` through the
    /// `load_file` Err arm with a parse error rather than a semantic one.
    /// spec: POL-50
    #[test]
    fn policy_review_malformed_toml_is_hard() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(&policy_path, "[sources\nallow = [").unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert_eq!(
            result.hard.len(),
            1,
            "malformed TOML must be exactly one hard finding: {:?}",
            result.hard
        );
        assert_eq!(result.hard[0].kind, "invalid-policy");
        assert!(
            result.advisory.is_empty(),
            "a parse failure yields no advisories: {:?}",
            result.advisory
        );
    }

    /// An `auto_meld` entry declaring two pins (tag + ref) is rejected at parse
    /// time (POL-5) and surfaces as a hard `invalid-policy` finding via review.
    /// spec: POL-50
    #[test]
    fn policy_review_two_pins_is_hard() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(
            &policy_path,
            "[[sources.auto_meld]]\nrepo = \"github.com/acme/a\"\ntag = \"v1\"\nref = \"abc123\"\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            result.hard.iter().any(|f| f.kind == "invalid-policy"),
            "two-pin auto_meld must be a hard finding: {:?}",
            result.hard
        );
    }

    /// A valid-but-empty policy (every control off, no auto_meld) reviews
    /// completely clean: zero hard, zero advisory.
    /// spec: POL-50
    #[test]
    fn policy_review_empty_policy_is_clean() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(&policy_path, "").unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            result.hard.is_empty(),
            "empty policy must have no hard findings: {:?}",
            result.hard
        );
        assert!(
            result.advisory.is_empty(),
            "empty policy must have no advisory findings: {:?}",
            result.advisory
        );
    }

    /// `lock = true` with an empty `allow` is a valid policy but an advisory:
    /// every meld would be blocked. It is NOT a hard finding, so dispatch
    /// returns Ok.
    /// spec: POL-50
    #[test]
    fn policy_review_lock_without_allow_is_advisory_only() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        // lock=true, no allow, no auto_meld: validate() passes, advisory fires.
        std::fs::write(&policy_path, "[sources]\nlock = true\n").unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            result.hard.is_empty(),
            "lock-without-allow must not be hard: {:?}",
            result.hard
        );
        assert!(
            result
                .advisory
                .iter()
                .any(|f| f.kind == "lock-without-allow"),
            "expected lock-without-allow advisory: {:?}",
            result.advisory
        );
        // An advisory-only result is not a failure: dispatch returns Ok.
        dispatch_policy(&policy_path)
            .expect("advisory-only policy must dispatch Ok (advisories never fail)");
    }

    /// An `auto_meld` entry present with `pinned = false` and `lock = false`
    /// floats freely: an advisory (`unpinned-auto-meld`), not a hard finding.
    /// The policy is valid (no invariant applies when neither flag is set).
    /// spec: POL-50
    #[test]
    fn policy_review_unpinned_auto_meld_is_advisory_only() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(
            &policy_path,
            "[[sources.auto_meld]]\nrepo = \"github.com/acme/baseline\"\n",
        )
        .unwrap();

        let result = review_policy(&policy_path).unwrap();
        assert!(
            result.hard.is_empty(),
            "unpinned floating auto_meld must not be hard: {:?}",
            result.hard
        );
        assert!(
            result
                .advisory
                .iter()
                .any(|f| f.kind == "unpinned-auto-meld"),
            "expected unpinned-auto-meld advisory: {:?}",
            result.advisory
        );
        dispatch_policy(&policy_path).expect("advisory-only policy must dispatch Ok");
    }

    /// `dispatch_policy` contract, success arm: a valid policy returns Ok(())
    /// and does not raise ReviewFailed. Drives the real dispatch path that the
    /// CLI routes `review --policy` through.
    /// spec: POL-50
    #[test]
    fn dispatch_policy_valid_returns_ok() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(
            &policy_path,
            "[sources]\nallow = [\"github.com/acme/*\"]\nlock = true\n\
             [[sources.auto_meld]]\nrepo = \"acme/baseline\"\n",
        )
        .unwrap();

        assert!(
            dispatch_policy(&policy_path).is_ok(),
            "a valid policy must dispatch Ok with no error"
        );
    }

    /// `dispatch_policy` contract, failure arm: a policy with a hard finding
    /// returns `Err(ReviewFailed { hard })` with the hard count, which is what
    /// drives the non-zero process exit. Here the file does not exist, so
    /// load_file errors -> one hard finding.
    /// spec: POL-50
    #[test]
    fn dispatch_policy_hard_returns_review_failed() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("missing-policy.toml");
        // Deliberately do not create the file: load_file returns an io error,
        // which review_policy maps to a single hard invalid-policy finding.

        match dispatch_policy(&policy_path) {
            Err(MindError::ReviewFailed { hard }) => {
                assert_eq!(hard, 1, "one hard finding => ReviewFailed.hard == 1");
            }
            other => panic!("expected Err(ReviewFailed), got {other:?}"),
        }
    }

    /// `dispatch_policy` failure arm with a semantic (validate) hard error:
    /// pinned=true + unpinned auto_meld (POL-21) makes load_file Err, dispatch
    /// returns ReviewFailed. Confirms `--policy` actually runs validate(), not
    /// just a parse. spec: POL-50
    #[test]
    fn dispatch_policy_validate_failure_returns_review_failed() {
        let tmp = TmpDir::new();
        let policy_path = tmp.path().join("policy.toml");
        std::fs::write(
            &policy_path,
            "[sources]\npinned = true\n\
             [[sources.auto_meld]]\nrepo = \"github.com/acme/baseline\"\n",
        )
        .unwrap();

        match dispatch_policy(&policy_path) {
            Err(MindError::ReviewFailed { hard }) => {
                assert_eq!(hard, 1, "the POL-21 invariant is one hard finding");
            }
            other => panic!("expected Err(ReviewFailed) from validate failure, got {other:?}"),
        }
    }

    // --- tooling / path-reference checks + --fix (CLI-135..138) ---

    /// An unresolved path token (`{{tools:nope}}`) is a hard bad-reference, just
    /// like an unresolved `{{ns:}}`.
    /// spec: CLI-135
    #[test]
    fn bad_path_token_is_hard_error() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        write_file(
            &source_dir.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\nrun {{tools:nope}} .\n",
        );
        let paths = paths_for(tmp.path());

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.iter().any(|f| f.kind == "bad-reference"),
            "an unresolved path token must be a hard bad-reference: {:?}",
            result.hard
        );
    }

    /// A hardcoded install path is an advisory that names the suggested token.
    /// spec: CLI-136
    #[test]
    fn hardcoded_path_is_advisory_with_suggestion() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        write_file(
            &source_dir.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\nrun ~/.claude/skills/review/resources/pr.py here\n",
        );
        let paths = paths_for(tmp.path());

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "hardcoded path is advisory, not hard"
        );
        let f = result
            .advisory
            .iter()
            .find(|f| f.kind == "hardcoded-path")
            .expect("expected a hardcoded-path advisory");
        assert!(
            f.message.contains("{{self}}/resources/pr.py"),
            "advisory must suggest the token: {}",
            f.message
        );
    }

    /// A sibling tool named in prose without a token is an advisory, even with no
    /// prefix in effect.
    /// spec: CLI-137
    #[test]
    fn bare_tool_reference_is_advisory() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        write_file(&source_dir.join("tools/detect/detect"), "#!/bin/sh\n");
        write_file(
            &source_dir.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\nFirst run the detect helper, then review.\n",
        );
        let paths = paths_for(tmp.path());

        // No prefix: the bare-tool advisory still fires (unlike unguarded-reference).
        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        assert!(
            result.hard.is_empty(),
            "must be hard-clean: {:?}",
            result.hard
        );
        let f = result
            .advisory
            .iter()
            .find(|f| f.kind == "bare-tool-reference")
            .expect("expected a bare-tool-reference advisory");
        assert!(
            f.message.contains("detect"),
            "names the tool: {}",
            f.message
        );
    }

    /// `--fix` on a local source rewrites hardcoded paths into tokens and bare
    /// sibling names into `{{ns:}}`, reporting the changed file.
    /// spec: CLI-138
    #[test]
    fn fix_rewrites_local_source() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        let skill = source_dir.join("skills/review/SKILL.md");
        write_file(
            &skill,
            "---\ndescription: review\n---\nrun ~/.claude/skills/review/run.sh; hand off to dev\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\ndescription: dev\n---\n# dev\n",
        );
        let paths = paths_for(tmp.path());

        let result = run_checks(&paths, &source_dir, None, true, true).unwrap();
        assert!(!result.fixed.is_empty(), "a file should be reported fixed");
        let rewritten = std::fs::read_to_string(&skill).unwrap();
        assert!(
            rewritten.contains("{{self}}/run.sh"),
            "hardcoded path must become a token: {rewritten}"
        );
        assert!(
            rewritten.contains("{{ns:dev}}"),
            "bare sibling name must be templatized: {rewritten}"
        );
    }

    /// `--fix` against a non-local target refuses and changes nothing.
    /// spec: CLI-138
    #[test]
    fn fix_refuses_non_local_target() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        let skill = source_dir.join("skills/review/SKILL.md");
        let original = "---\ndescription: review\n---\nrun ~/.claude/skills/review/run.sh\n";
        write_file(&skill, original);
        let paths = paths_for(tmp.path());

        // is_local = false (a registry/remote target): --fix must refuse.
        let result = run_checks(&paths, &source_dir, None, true, false).unwrap();
        assert!(
            result.hard.iter().any(|f| f.kind == "fix-not-local"),
            "non-local --fix must produce a fix-not-local hard finding: {:?}",
            result.hard
        );
        assert!(result.fixed.is_empty(), "nothing should be reported fixed");
        assert_eq!(
            std::fs::read_to_string(&skill).unwrap(),
            original,
            "the file must be unchanged"
        );
    }

    /// A `{{ns:}}` token in a code block / span / path is an advisory
    /// misplaced-reference; one in the frontmatter `name:` field is hard.
    /// spec: CLI-139
    #[test]
    fn misplaced_ns_tokens_are_flagged() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        // `do` and `dev` are siblings, so the tokens resolve (not bad-reference);
        // they are still misplaced by context.
        write_file(
            &source_dir.join("agents/do.md"),
            "---\nname: do\n---\n# do\n",
        );
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\nname: dev\n---\n# dev\n",
        );
        write_file(
            &source_dir.join("agents/lead.md"),
            "---\nname: {{ns:lead}}\n---\nrun `{{ns:do}}` then see ~/{{ns:dev}}\n",
        );
        write_file(
            &source_dir.join("agents/lead2.md"),
            "---\nname: lead\n---\nx\n",
        );
        let paths = paths_for(tmp.path());

        let result = run_checks(&paths, &source_dir, None, false, true).unwrap();
        // Frontmatter name token -> hard.
        assert!(
            result
                .hard
                .iter()
                .any(|f| f.kind == "misplaced-reference" && f.message.contains("name:")),
            "frontmatter name token must be hard: {:?}",
            result.hard
        );
        // Code-span and path tokens -> advisory.
        let adv = result
            .advisory
            .iter()
            .filter(|f| f.kind == "misplaced-reference")
            .count();
        assert!(
            adv >= 2,
            "code-span + path tokens must be advisory: {:?}",
            result.advisory
        );
    }

    /// `--fix` un-wraps misplaced `{{ns:}}` tokens back to bare words.
    /// spec: CLI-138 CLI-139
    #[test]
    fn fix_unwraps_misplaced_ns_tokens() {
        let tmp = TmpDir::new();
        let source_dir = tmp.path().join("src");
        write_file(
            &source_dir.join("agents/dev.md"),
            "---\nname: dev\n---\n# dev\n",
        );
        let lead = source_dir.join("agents/lead.md");
        write_file(
            &lead,
            "---\nname: {{ns:lead}}\n---\npath ~/{{ns:dev}} and `{{ns:dev}}`\n",
        );
        let paths = paths_for(tmp.path());

        let result = run_checks(&paths, &source_dir, None, true, true).unwrap();
        assert!(!result.fixed.is_empty());
        let fixed = std::fs::read_to_string(&lead).unwrap();
        assert!(
            fixed.contains("name: lead"),
            "frontmatter name un-wrapped: {fixed}"
        );
        assert!(
            fixed.contains("~/dev") && fixed.contains("`dev`"),
            "path/span un-wrapped: {fixed}"
        );
    }

    /// Assert no `review-*` scratch dir survives under MIND_HOME/.tmp.
    fn assert_no_review_temp(paths: &Paths) {
        let tdir = paths.tmp_dir();
        if !tdir.exists() {
            return;
        }
        for entry in std::fs::read_dir(&tdir).unwrap().flatten() {
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().starts_with("review-"),
                "leftover review temp dir: {:?}",
                entry.path()
            );
        }
    }
}
