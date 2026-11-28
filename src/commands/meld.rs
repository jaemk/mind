//! `mind meld`, nested and curated sources, and source namespaces.

use super::*;

/// `mind meld <repo> [--as <prefix>] [--root <dir>] [--follow-branch|--pin-tag|--pin-ref]`
/// — register and clone a source.
///
/// If the source's `mind.toml` lists nested `[discover].sources`, each is melded
/// too (recursively), so a repo can act as a curated super-source. Nested
/// sources are skipped if already registered, and cycles are guarded by URL.
///
/// Returns a `MeldSummary` with the data needed for the caller to emit a combined
/// JSON object (CLI-156): the dispatcher folds the post-meld install outcome into
/// ONE JSON result rather than letting each step emit separately.
#[allow(clippy::too_many_arguments)]
pub fn meld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    add_roots: Vec<String>,
    flat_skills: bool,
    pin: PinRequest,
    install_hook: Option<String>,
    accept: crate::hook::Accept,
    item_kind: Option<ItemKind>,
) -> Result<MeldSummary> {
    meld_with_profiles(
        paths,
        repo,
        alias,
        roots,
        add_roots,
        flat_skills,
        pin,
        install_hook,
        accept,
        item_kind,
        &mut MeldProfiles::None,
        false,
        false,
    )
}

/// [`meld`] followed by a consumer profile install (HOOK-142): `profiles`
/// resolves the selection before the source's install hooks run.
/// `accept_newer_mind` is `meld --yes`: meld a source whose
/// `min-mind-version` this binary does not meet without asking (DSC-116).
#[allow(clippy::too_many_arguments)]
pub(crate) fn meld_with_profiles(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    add_roots: Vec<String>,
    flat_skills: bool,
    pin: PinRequest,
    install_hook: Option<String>,
    accept: crate::hook::Accept,
    item_kind: Option<ItemKind>,
    profiles: &mut MeldProfiles<'_>,
    accept_newer_mind: bool,
    full_history: bool,
) -> Result<MeldSummary> {
    // spec: HOME-3 -- a meld into another home scaffolds that home only.
    match crate::paths::meld_home().home {
        Some(home) => crate::paths::mkdir_p(&home)?,
        None => paths.ensure_layout()?,
    }
    // POL-3: the managed policy is authoritative over user intent. Load it once
    // (Err = invalid policy, fail closed via `?`; None = unmanaged, inert).
    let policy = paths.settings().policy()?;
    // CLI-19: prefer SSH for remotes when the user's config asks for it.
    let prefer_ssh = paths.settings().config()?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut visited = HashSet::new();
    // spec: STO-58 -- the reported identity includes the consumer alias, so the
    // deferred JSON install and the caller's post-meld steps target this instance.
    // spec: CLI-216 -- an identity-only parse: `meld_recursive` below performs
    // the parse that really decides which reading gets cloned, and that one
    // carries the CLI-215 note.
    let source_name = parse_spec_quiet(repo)
        .map(|mut s| {
            s.apply_alias(alias.clone());
            s.name
        })
        .unwrap_or_else(|_| repo.to_string());
    let mut meld_skipped: Vec<SkippedEntry> = Vec::new();
    // A top-level meld has no curator or curator configuration, and does not
    // yet thread --yes (non-TTY always fires NS-45).
    let added = meld_recursive(
        &mut MeldState {
            paths,
            registry: &mut registry,
            visited: &mut visited,
            policy: policy.as_ref(),
            accept,
            prefer_ssh,
            skipped: &mut meld_skipped,
            accept_newer_mind,
        },
        MeldRequest {
            alias,
            roots,
            add_roots,
            flat_skills,
            consumer_pin: pin,
            top_level: true,
            install_hook,
            item_kind,    // spec: CLI-239 -- the consumer's `--kind` for a file link
            full_history, // spec: DSC-121 -- the named source only, not what it curates
            ..MeldRequest::new(repo, profiles)
        },
    )?;
    registry.save(paths)?;
    // JSON emission is deferred to the dispatcher (main.rs) so the install
    // outcome can be folded into ONE object (CLI-156). Human output is still
    // printed here because it is unrelated to the install step.
    let out = crate::render::ctx();
    if !out.json && added > 1 {
        println!("melded {added} source(s)");
    }
    Ok(MeldSummary {
        source_name,
        added,
        skipped: meld_skipped,
    })
}

/// DSC-56: after melding a source that curates other sources (`[discover].sources`),
/// point the user at `mind probe` to browse what is now available. No-op for a
/// source with no nested sources, and silent under `--json`.
pub fn maybe_probe_hint(paths: &Paths, source_name: &str) -> Result<()> {
    let out = crate::render::ctx();
    if out.json {
        return Ok(());
    }
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(());
    };
    // spec: LNK-8 -- an item-link instance adopts no curator layer, so the
    // browse hint would be misleading.
    if source.item_path.is_some() {
        return Ok(());
    }
    let clone_dir = source.clone_dir(paths);
    let curates = MindToml::load(&clone_dir)?
        .and_then(|m| m.discover)
        .is_some_and(|d| !d.sources.is_empty());
    // MKT-7: a marketplace source is also a curated super-source; fire the
    // probe hint so the user knows to browse what became available.
    let has_marketplace = plugin_manifest::find_marketplace_manifest(&clone_dir).is_some();
    if curates || has_marketplace {
        println!(
            "note: this source curates other sources; run `mind probe` to browse and search what is available"
        );
    }
    Ok(())
}

/// Stamp a record's provenance after [`record_install_hook`] created it, for a
/// hook that did not come from the source's own manifest (HOOK-56, HOOK-127).
/// A no-op for an ordinary declared hook.
pub(super) fn stamp_origin(
    source: &mut crate::source::Source,
    hook: &crate::mindfile::ResolvedHook,
    event: RecordedEvent,
    origin: Option<HookOrigin>,
) {
    if origin.is_none() {
        return;
    }
    record_source_hook_entry(source, hook, event, None, origin, false);
}

/// Curator-supplied configuration for a nested source, lifted from a parent
/// super-source's `[discover].sources` entry (DSC-59). Resolved by the parent
/// before recursing; the pin is always applied (DSC-65, authoritative); roots,
/// add-roots, flat-skills, and hooks are gated (DSC-60): applied only while
/// the nested source's own `mind.toml`, if any, declares no discovery or hooks
/// ([`crate::mindfile::MindToml::gates_curated_config`]).
#[derive(Default)]
pub(crate) struct CuratedConfig {
    /// The curator's `mind-home` for this entry (HOME-10). Not gated: it picks
    /// the home like `meld --home` does, before the clone.
    pub(super) mind_home: Option<String>,
    /// The curator pin directive (follow-branch, pin-tag, or pin-ref), if set.
    /// Authoritative: applied whether or not the nested source has a mind.toml
    /// (DSC-65). NOT included in the DSC-60 gating/warning.
    pub(super) pin: Option<Pin>,
    /// The curator `roots`, if set (an explicit empty list is preserved).
    /// Gated by DSC-60.
    pub(super) roots: Option<Vec<String>>,
    /// The curator `add-roots` (DSC-88), if set. Gated by DSC-60/DSC-88, same
    /// as `roots` -- an authoritative nested manifest is an export-control
    /// decision (DSC-60) and a curator's `add-roots` must not bypass it.
    pub(super) add_roots: Option<Vec<String>>,
    /// The curator `flat-skills` (DSC-77). Gated by DSC-60.
    pub(super) flat_skills: bool,
    /// The curator `[[discover.sources.hooks]]`, resolved in declaration order.
    /// Gated by DSC-60.
    pub(super) hooks: Vec<crate::mindfile::ResolvedHook>,
}

impl CuratedConfig {
    /// Whether any DSC-60-GATED values (roots, add-roots, or hooks) are
    /// present, so the "ignored" warning is warranted when the nested source
    /// has its own `mind.toml`. The pin is NOT gated (DSC-65) and does NOT
    /// participate.
    fn has_gated_values(&self) -> bool {
        self.roots.is_some()
            || self.add_roots.is_some()
            || self.flat_skills
            || !self.hooks.is_empty()
    }
}

/// Lift a `[discover].sources` entry's curator-supplied configuration (DSC-59)
/// into the shape `meld_recursive` applies.
///
/// One definition for all three readers of a curator's list: the nested loop in
/// `meld_recursive`, `sync`'s DSC-57 re-walk, and `curate`'s plan (curate.md
/// CUR-3). Without it, a key added to the entry shape has to be threaded into
/// each site by hand, and a site that forgets it silently drops the curator's
/// directive.
///
/// spec: DSC-88 -- `add-roots` is GATED (DSC-60), same as roots/flat-skills/
/// hooks: it must not bypass a nested source's authoritative export list
/// (DSC-3). Routed through `CuratedConfig` (not the consumer `--add-root`
/// slot) so `meld_recursive` applies it only while the DSC-60 gate is open.
pub(crate) fn curated_config_for(
    entry: &crate::mindfile::NestedSource,
    toml_path: &std::path::Path,
) -> Result<CuratedConfig> {
    Ok(CuratedConfig {
        mind_home: entry.mind_home.clone(),
        pin: entry.pin_directive(toml_path)?,
        roots: entry.roots.clone(),
        add_roots: entry.add_roots.clone(),
        flat_skills: entry.flat_skills,
        hooks: entry.resolved_hooks(toml_path)?,
    })
}

/// State shared by every frame of one [`meld_recursive`] walk: the registry
/// being built, the cycle guard, the skipped-entry report, and the settings
/// every nested source inherits from the meld that started the walk.
pub(crate) struct MeldState<'a> {
    pub(crate) paths: &'a Paths,
    pub(crate) registry: &'a mut Registry,
    /// Cycle guard keyed on `identity|url` (LNK-4).
    pub(crate) visited: &'a mut HashSet<String>,
    pub(crate) policy: Option<&'a Policy>,
    pub(crate) accept: crate::hook::Accept,
    /// CLI-19: rewrite https remotes to SSH.
    pub(crate) prefer_ssh: bool,
    /// Nested entries skipped with a reason (DSC-79).
    pub(crate) skipped: &'a mut Vec<SkippedEntry>,
    /// The consumer's `meld --yes` answer to the DSC-116 question, carried to
    /// the sources this meld curates.
    pub(crate) accept_newer_mind: bool,
}

/// One source for [`meld_recursive`] to meld. [`MeldRequest::new`] gives the
/// defaults of a nested meld (no consumer overrides, not top-level); a caller
/// sets only the fields it supplies.
pub(crate) struct MeldRequest<'a, 'p> {
    pub(crate) repo: &'a str,
    pub(crate) alias: Option<String>,
    /// The consumer `--root` override (empty => no override).
    pub(crate) roots: Vec<String>,
    pub(crate) add_roots: Vec<String>,
    pub(crate) flat_skills: bool,
    /// The caller-supplied pin (CLI flags, or none for a nested source).
    pub(crate) consumer_pin: PinRequest,
    /// The user's own meld (errors on a duplicate) rather than a curated
    /// nested meld (skips a duplicate).
    pub(crate) top_level: bool,
    pub(crate) install_hook: Option<String>,
    /// Mirrors the CLI `--yes` flag (NS-45): forces non-interactive mode (the
    /// NS-43 collision prompt is skipped and hard-errors instead).
    pub(crate) yes: bool,
    /// The curator-supplied configuration from a parent super-source's
    /// `[discover].sources` entry (DSC-59): a pin, scan roots, and lifecycle
    /// hooks. The gated ones apply only while the nested source's `mind.toml`
    /// declares no discovery or hooks (DSC-60); otherwise they are ignored
    /// with a warning.
    pub(crate) curated: Option<CuratedConfig>,
    /// The consumer's or curator's explicit kind for a file link (CLI-239, DSC-100).
    pub(crate) item_kind: Option<ItemKind>,
    /// The curator whose list registered this source (STO-82).
    pub(crate) curated_by: Option<String>,
    pub(crate) profiles: &'a mut MeldProfiles<'p>,
    /// `--full-history` (DSC-121): the named source only, never what it curates.
    pub(crate) full_history: bool,
}

impl<'a, 'p> MeldRequest<'a, 'p> {
    pub(crate) fn new(repo: &'a str, profiles: &'a mut MeldProfiles<'p>) -> Self {
        MeldRequest {
            repo,
            alias: None,
            roots: vec![],
            add_roots: vec![],
            flat_skills: false,
            consumer_pin: PinRequest::None,
            top_level: false,
            install_hook: None,
            yes: false,
            curated: None,
            item_kind: None,
            curated_by: None,
            profiles,
            full_history: false,
        }
    }
}

fn curated_identity(repo: &str, alias: Option<String>) -> Option<String> {
    let mut source = parse_spec_quiet(repo).ok()?;
    source.apply_alias(alias);
    Some(source.name)
}

/// Meld one source and then its nested sources. Returns how many sources were
/// newly added to the registry.
pub(crate) fn meld_recursive(st: &mut MeldState<'_>, req: MeldRequest<'_, '_>) -> Result<usize> {
    meld_with_direct_sources(st, req, &HashSet::new())
}

fn meld_with_direct_sources(
    st: &mut MeldState<'_>,
    req: MeldRequest<'_, '_>,
    direct_sources: &HashSet<String>,
) -> Result<usize> {
    let paths = st.paths;
    let policy = st.policy;
    let accept = st.accept;
    let prefer_ssh = st.prefer_ssh;
    let accept_newer_mind = st.accept_newer_mind;
    let MeldRequest {
        repo,
        alias,
        roots,
        add_roots,
        flat_skills,
        consumer_pin,
        top_level,
        install_hook,
        yes,
        curated,
        item_kind,
        curated_by,
        profiles,
        full_history,
    } = req;
    let out = crate::render::ctx();
    let mut source = parse_spec(repo)?;
    // spec: DSC-121 -- the consumer's `--full-history`, honored at the clone
    // below and by every later sync.
    source.full_history = full_history;
    // spec: DSC-100 -- a `kind` on an entry whose `source` is not an item link
    // is a mismatch, caught before any clone: `kind` only ever describes a
    // file link's item. The CLI's own `--kind` cannot reach here with a
    // non-link spec (`parse_link_kind` already refused it as `BadKindFlag`);
    // this is what closes the same gap for a curator's `kind =`, which has no
    // such upfront check of its own.
    if let Some(kind) = item_kind
        && source.item_path.is_none()
    {
        return Err(MindError::LinkKindMismatch {
            source_name: source.name.clone(),
            path: repo.to_string(),
            kind: kind.as_str().to_string(),
            reason: "the source is not an item link; `kind` only applies to a deep \
                     tree/blob URL entry"
                .to_string(),
        });
    }
    // spec: LNK-22 CLI-239 DSC-100 -- the consumer's (or curator's) explicit
    // kind for a file link, recorded on the instance and read by every later
    // scan. Applied before the catalog scan below, which resolves the kind.
    source.item_kind = item_kind;
    // spec: STO-82 -- provenance: which curator's list this registration came
    // from. Recorded unconditionally (it is not curator CONFIGURATION, so the
    // DSC-60 gate that gags roots/hooks/flat-skills does not apply to it), and
    // read back by `curate` to tell an unlisted source from a directly melded
    // one (curate.md CUR-7).
    source.curated_by = curated_by;
    // NS-25: reject a reserved-kind-word prefix at the chokepoint where any alias
    // (top-level `--as`, or a nested source's `as =`) is applied. An empty alias
    // ("no prefix") is accepted by validate_prefix.
    if let Some(a) = &alias {
        crate::namespace::validate_prefix(a)?;
    }
    // spec: STO-58 -- the alias is part of the source identity, so applying it
    // folds `@<alias>` into `name` (composing with an item-link `#<path>`). This
    // is what makes `meld <repo> --as <prefix>` a distinct instance that coexists
    // with the bare repo and with other aliases.
    source.apply_alias(alias);
    // CLI-19: rewrite an https remote to its SSH form when SSH is preferred, so
    // the clone uses the user's key (no https username/password prompt). Done
    // before the cycle guard so the recorded URL is the one we actually clone.
    source.prefer_ssh(prefer_ssh);

    // spec: LNK-3 -- an item link carries its pin in the URL ref; lift it so
    // the standard pin resolution below can layer a consumer flag over it and
    // the pre-pin code sees the default (a link is never a linked local tree).
    let url_pin = if source.item_path.is_some() && source.pin != Pin::DefaultBranch {
        Some(std::mem::replace(&mut source.pin, Pin::DefaultBranch))
    } else {
        None
    };

    // Cycle guard: don't process the same source twice in one meld run. Keyed
    // on identity + URL: item-link instances share their repo's URL but are
    // distinct sources (LNK-4), while a same-name re-spec still reaches the
    // registry check below for its "already melded" warning.
    if !st.visited.insert(format!("{}|{}", source.name, source.url)) {
        return Ok(0);
    }

    if let Some(existing) = st.registry.find(&source.name) {
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

    // spec: HOME-4 HOME-6 -- a new source goes into the home `meld --home`
    // named, or its curator's home; its own `[source] mind-home` may move it
    // once the clone is read (below). A curated source follows its curator's
    // home on every path that registers it: the first meld, `sync`'s DSC-57
    // re-walk, and `curate`.
    // spec: HOME-10 -- a curator entry's `mind-home` picks the home the way
    // `meld --home` does: checked before the clone, outranking the source's
    // own `[source] mind-home`.
    let entry_home = curated.as_ref().and_then(|c| c.mind_home.as_deref());
    let meld_home = match (entry_home, source.curated_by.as_deref()) {
        (Some(h), curator) => {
            let origin = format!(
                "the `mind-home` of the entry for {} in {}'s mind.toml",
                strip_ansi(&source.name),
                strip_ansi(curator.unwrap_or("its curator"))
            );
            crate::paths::MeldHome {
                home: paths.allowed_home(h, &origin)?,
                explicit: true,
            }
        }
        (None, Some(curator)) => crate::paths::MeldHome {
            home: paths.source_home(curator),
            explicit: false,
        },
        (None, None) => crate::paths::meld_home(),
    };
    paths.set_source_home(&source.name, meld_home.home.as_deref());
    // spec: HOME-15 -- a home the consumer chose is kept on `upgrade`.
    source.chosen_home = meld_home.explicit;

    // `source.pin` is still the default here, so `clone_dir` resolves a local
    // source to its working tree. A consumer/directive pin (resolved below) can
    // still switch a local source to a cloned snapshot.
    let mut dir = clone_dir_checked(paths, &source)?;
    let is_local = source.is_local();

    if !out.json {
        println!(
            "{} melding {} from {}",
            out.bullet(),
            source.name,
            out.dim(&source.url)
        );
    }

    // spec: POL-36 -- the allow/lock check requires only the parsed source
    // identity (available now, before any clone). Hoisting it here ensures no
    // git network call or directory creation occurs for a refused source (E14).
    // The pinned check (POL-20) stays post-clone because it needs the effective
    // pin, which may come from the source's mind.toml (read after the clone).
    if let Some(policy) = policy {
        // spec: LNK-11 -- allowlist matching uses the base repo identity, so a
        // policy that allows the repo allows links into it.
        let identity = source.base_identity();

        // spec: POL-56 -- when allow-local is false under a lock, local-path
        // and file:// melds are refused regardless of allow patterns. This check
        // precedes the allow-pattern check so the error message names the
        // allow-local reason rather than a pattern miss.
        if policy.lock() && !policy.allow_local() && is_local {
            if let Some(path) = paths.settings().policy_path() {
                // spec: POL-37 (same hint pattern as SourceNotAllowed)
                eprintln!("hint: managed policy at {}", path.display());
            }
            return Err(MindError::LocalMeldForbidden { identity });
        }

        // spec: POL-70 -- a cleartext transport is refused before any clone,
        // with or without `lock`.
        if !policy.allow_insecure_transport()
            && let Some(scheme) = source.insecure_scheme()
        {
            if let Some(path) = paths.settings().policy_path() {
                // spec: POL-37
                eprintln!("hint: managed policy at {}", path.display());
            }
            return Err(MindError::InsecureTransportForbidden {
                identity,
                scheme: scheme.to_string(),
            });
        }

        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            // spec: POL-11, POL-36 -- refused before any clone or dir creation.
            // Print the policy file path so the developer knows where to look.
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
        // spec: CLI-177, CLI-178, CLI-180 -- for a top-level meld, intercept
        // clone failures to lead with git's stderr and emit auth/proxy hints.
        git::clone(&source.url, &dir).map_err(|e| {
            if top_level {
                let ssh = (!source.is_local()).then(|| source.ssh_url());
                handle_top_level_clone_err(e, ssh, &dir, out)
            } else {
                e
            }
        })?;
    }
    let mut mindfile = MindToml::load(&dir)?;
    // spec: HOME-5 -- the source's own `[source] mind-home`, unless `meld
    // --home` named one. It must be an allowed home; otherwise the meld is
    // refused and the clone removed.
    let declared_home = mindfile.as_ref().and_then(|m| m.source.mind_home.clone());
    if let Some(declared) = declared_home.filter(|_| !meld_home.explicit) {
        let origin = format!(
            "the `mind-home` in {}'s mind.toml",
            strip_ansi(&source.name)
        );
        let target = match paths.allowed_home(&declared, &origin) {
            Ok(t) => t,
            Err(e) => {
                if !is_local {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return Err(e);
            }
        };
        if target != meld_home.home {
            paths.set_source_home(&source.name, target.as_deref());
            if !is_local {
                dir = relocate_clone(paths, &source, &dir)?;
            }
        }
    }
    // spec: LOBE-2 -- a lobe that is not allowed refuses the meld before
    // anything is registered or installed, and the clone is removed. It is
    // checked against the home just settled: the consumer's `--lobe`, which
    // outranks the source's own (LOBE-7), else the source's `lobe`.
    let consumer_lobe = paths.consumer_lobe_decl(&source.name);
    let in_effect = match consumer_lobe.as_deref() {
        Some(l) => Some((l, true)),
        None => mindfile
            .as_ref()
            .and_then(|m| m.source.lobe.as_deref())
            // An empty `lobe` declares none, as everywhere else (LOBE-1).
            .filter(|l| !l.trim().is_empty())
            .map(|l| (l, false)),
    };
    if let Some((declared, consumer)) = in_effect
        && let Err(e) = paths.resolve_lobe(
            &source.name,
            declared,
            &crate::paths::lobe_origin(&source.name, consumer),
        )
    {
        if !is_local {
            let _ = std::fs::remove_dir_all(&dir);
        }
        return Err(e);
    }
    // spec: HOME-6 -- the sources this one curates default to its home.
    let _home_scope = crate::paths::MeldHomeScope::set(crate::paths::MeldHome {
        home: paths.source_home(&source.name),
        explicit: false,
    });
    // spec: DSC-94 -- B4: a `mind.toml [source].description` is source-
    // controlled text, sanitized here at the store point exactly like the
    // plugin-manifest path above (line ~1181), closing the frontmatter-path
    // gap for the biggest funnel of raw-ANSI/bidi-override source text.
    source.description = mindfile
        .as_ref()
        .and_then(|m| m.source.description.clone())
        .map(|d| strip_ansi(&d));

    // DSC-60: the curator-supplied roots, add-roots, flat-skills, and hooks
    // apply only while the nested source's own mind.toml (if any) declares
    // none of its own discovery or hooks. A mind.toml that does suppresses all
    // four. `as` (DSC-39) and `install` (DSC-58) are not gated. DSC-65: the
    // curator's pin directive is NOT gated -- it is authoritative regardless
    // of the nested mind.toml.
    let curated = curated.unwrap_or_default();
    let apply_curated = !mindfile.as_ref().is_some_and(|m| m.gates_curated_config());
    // The curator pin is always extracted (DSC-65: authoritative, not gated).
    let curated_pin = curated.pin.clone();
    if !apply_curated && curated.has_gated_values() {
        // spec: DSC-60/DSC-88 — warn only when gated fields (roots/
        // add-roots/flat-skills/hooks) are present and suppressed. A pin-only
        // entry must NOT trigger this warning. `add-roots` specifically has a
        // consumer-side escape (`meld --add-root`, DSC-84) that is NOT gated,
        // so a curator's suppressed `add-roots` need not silently install
        // fewer items than before: point the consumer at applying it
        // themselves.
        let add_root_hint = if curated.add_roots.is_some() {
            // spec: CLI-225 -- `source.name` is source-influenced (derived
            // from the repo spec / `--as` alias) and lands in a pasteable
            // `mind meld <name> --add-root <dir>` remedy, so it is
            // shell-quoted before printing.
            format!(
                "; to apply add-roots yourself, run `mind meld {} --add-root <dir>` \
                 (that flag is not gated by a nested mind.toml)",
                crate::error::shell_quote(&source.name)
            )
        } else {
            String::new()
        };
        eprintln!(
            "warning: {} declares its own discovery or hooks in its mind.toml; curator-supplied roots/add-roots/flat-skills/hooks are ignored{add_root_hint}",
            source.name
        );
    }
    let curated_hooks = if apply_curated {
        curated.hooks.clone()
    } else {
        Vec::new()
    };

    // Step 2: resolve the pin (CLI-17, DSC-41, DSC-65, CLI-200..202).
    // Lower-precedence layers select a floating base point:
    //   curator pin (DSC-65, authoritative) > item-link URL ref (LNK-3) >
    //   [source] directive (DSC-41) > DefaultBranch.
    let toml_path = dir.join("mind.toml");
    let directive_pin = mindfile
        .as_ref()
        .map(|m| m.source.pin_directive(&toml_path))
        .transpose()?
        .flatten();
    let base_pin = curated_pin.or(url_pin).or(directive_pin);

    // spec: VER-50 -- with no pin from any layer, a remote source whose
    // default-branch mind.toml declares release-tags pins the caret of its
    // highest stable release. A local path stays linked to its working tree.
    let mut default_release = false;
    let base_pin = match base_pin {
        Some(p) => p,
        None if consumer_pin == PinRequest::None && !is_local => {
            match default_version_pin(&source, mindfile.as_ref(), &toml_path, &dir) {
                Ok(Some(p)) => {
                    default_release = true;
                    p
                }
                Ok(None) => Pin::DefaultBranch,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            }
        }
        None => Pin::DefaultBranch,
    };

    // The consumer flag layers over the base: `--pin branch=`/`tag=` overrides the
    // point (follow), a `--pin` freeze value fixes a point to its current commit
    // (CLI-200/201). `checkout_pin` is what we actually check out; `freeze` records
    // whether the resolved commit is then persisted as an immutable ref.
    //   - `--pin HEAD` freezes the base the lower layers chose;
    //   - `--pin <ref>` names its own point (branch, tag, or sha), checked out via
    //     the ref path (`git checkout <ref>` accepts all three) and frozen.
    let (checkout_pin, freeze) = resolve_checkout_pin(consumer_pin, base_pin);

    // spec: VER-42 -- a version pin resolves now, against the default branch's
    // release-tags (VER-3, VER-4), and fails the meld when nothing is in range.
    // The VER-50 default arrives already resolved.
    let checkout_pin = match checkout_pin {
        Pin::Version(vp) if vp.resolved.is_none() => {
            let cleanup = |dir: &std::path::Path| {
                if !is_local {
                    let _ = std::fs::remove_dir_all(dir);
                }
            };
            let meta = mindfile
                .as_ref()
                .map(|m| crate::mindfile::ReleaseMeta::from_source(&m.source, &toml_path));
            let meta = match meta.transpose() {
                Ok(m) => m.unwrap_or_default(),
                Err(e) => {
                    cleanup(&dir);
                    return Err(e);
                }
            };
            let template = meta.template.unwrap_or(crate::version::Template::Default);
            let yanked = meta.yanked;
            let vp = crate::source::VersionPin {
                recommended: meta.recommended,
                auto_update: meta.auto_update,
                ..vp
            };
            let res = resolve_version(&source.name, &source.url, &vp, &template, &yanked).and_then(
                |mut res| {
                    fit_min_mind_version(&source.name, &source.url, &dir, !is_local, &mut res)?;
                    Ok(res)
                },
            );
            let res = match res {
                Ok(r) => r,
                Err(e) => {
                    cleanup(&dir);
                    return Err(e);
                }
            };
            match &res.selected {
                Some(rel) => Pin::Version(filled_version_pin(&vp, rel, &res)),
                None => {
                    cleanup(&dir);
                    return Err(no_matching_release(&source.name, &vp, &res));
                }
            }
        }
        other => other,
    };

    // spec: POL-20 -- pinned check stays post-clone because it needs the effective
    // pin, which may come from the source's mind.toml. Evaluated on the FINAL
    // persisted kind: a `--pin` freeze persists a ref, so it satisfies
    // require-pinned even when the point it freezes is an otherwise-floating branch.
    // spec: VER-100 -- an exact version is pinned; a range only under
    // `allow-version-ranges`.
    if let Some(policy) = policy {
        let identity = source.name.clone();
        if !freeze && policy.forbids_unpinned(&checkout_pin) {
            // POL-20: pinned policy forbids a floating branch (default branch or
            // follow-branch); only a tag / ref / frozen pin is permitted.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::UnpinnedSourceForbidden { identity });
        }
    }

    // spec: VER-52 -- once the policy admits it, say that the meld follows
    // releases rather than the default branch, and how later releases arrive.
    if default_release && let Pin::Version(vp) = &checkout_pin {
        eprintln!(
            "note: {} follows releases {} (now {}); `mind upgrade` takes compatible releases and asks before a new major",
            strip_ansi(&source.name),
            vp.range,
            vp.resolved.as_deref().unwrap_or("?")
        );
    }

    // Step 3: if the checkout point is not DefaultBranch, the source is a clone at
    // that point -- including a *pinned local* source, which is snapshotted into
    // the sources tree (so pinning still works) WITHOUT touching the working tree.
    // A pin that does not resolve is a `Git` error that must leave nothing behind
    // (CLI-18); the clone target is always under the sources tree, never the user's
    // working tree.
    if checkout_pin != Pin::DefaultBranch {
        // Setting the pin makes `clone_dir` resolve to the sources-tree path even
        // for a local source.
        source.pin = checkout_pin.clone();
        let target = clone_dir_checked(paths, &source)?;
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&source.url, &target, &checkout_pin) {
            let _ = std::fs::remove_dir_all(&target);
            // spec: CLI-177, CLI-178, CLI-180 -- apply the same top-level clone
            // error handling (lead with stderr, auth/proxy hints, strip store path)
            // for the re-clone-at-pin step.
            let e = if top_level {
                let ssh = (!source.is_local()).then(|| source.ssh_url());
                handle_top_level_clone_err(e, ssh, &target, out)
            } else {
                e
            };
            return Err(e);
        }
        dir = target;
        // The pin may land on a different mind.toml than the working tree / default
        // branch. Reload it so downstream in-memory reads see the pinned content.
        mindfile = MindToml::load(&dir)?;
        // spec: DSC-94 -- B4: sanitize at every re-read of the pinned mind.toml too.
        source.description = mindfile
            .as_ref()
            .and_then(|m| m.source.description.clone())
            .map(|d| strip_ansi(&d));
    }

    // Resolve the recorded commit at the checkout point. A linked (no-pin) local
    // source records its working-tree HEAD (best-effort; a non-git local dir
    // simply records no commit). Everything else records the cloned commit. A
    // freeze always needs a concrete commit, so it never takes the best-effort path.
    let linked_default = is_local && checkout_pin == Pin::DefaultBranch && !freeze;
    let commit = if linked_default {
        git::head_commit(&source.url, &dir).ok()
    } else {
        Some(git::head_commit(&source.url, &dir)?)
    };

    // Step 4: a freeze persists the resolved commit as an immutable ref (CLI-200).
    // When the frozen point is a *local* source's default branch, Step 3 did not
    // clone (the working tree was read live), so snapshot it into the sources tree
    // now -- a ref-pinned local source is a clone, never a live working tree (CLI-27).
    let final_pin = if freeze {
        let sha = commit.clone().expect("a freeze reads a concrete commit");
        let ref_pin = Pin::Ref(sha);
        if is_local && checkout_pin == Pin::DefaultBranch {
            source.pin = ref_pin.clone();
            let target = clone_dir_checked(paths, &source)?;
            if target.exists() {
                std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
            }
            if let Some(parent) = target.parent() {
                crate::paths::mkdir_p(parent)?;
            }
            // spec: CLI-18 -- a failed snapshot must leave nothing behind, same as
            // the Step-3 clone. The source (local) is not a network remote, so the
            // top-level stderr/auth handling does not apply here.
            if let Err(e) = git::clone_at(&source.url, &target, &ref_pin) {
                let _ = std::fs::remove_dir_all(&target);
                return Err(e);
            }
            dir = target;
            mindfile = MindToml::load(&dir)?;
            // spec: DSC-94 -- B4: sanitize at the freeze-snapshot re-read too.
            source.description = mindfile
                .as_ref()
                .and_then(|m| m.source.description.clone())
                .map(|d| strip_ansi(&d));
        }
        ref_pin
    } else {
        checkout_pin
    };

    source.pin = final_pin;
    source.commit = commit;
    // spec: DSC-121 -- deepen before anything (a hook, DSC-121's motivating
    // case) runs against the clone. A linked local source is the user's own
    // working tree: say so rather than silently doing nothing.
    if source.full_history && source.is_linked() {
        eprintln!("{}", full_history_linked_note(&source.name));
    }
    honor_full_history(&source, &dir)?;

    // spec: DSC-116 -- a source that declares a newer `min-mind-version` may
    // not work under this binary. Warn and ask before registering it; with no
    // terminal to ask on, only an explicit `--yes` carries on.
    if let Some(required) = crate::mindfile::unmet_min_version(mindfile.as_ref()) {
        let required = required.to_string();
        // `--json` is non-interactive (LIFE-45): a prompt would corrupt its stdout.
        let interactive = !yes && !json_mode() && crate::hook::is_tty();
        if accept_newer_mind || interactive {
            eprintln!("{}", catalog::newer_mind_warning(&source.name, &required));
            catalog::first_version_warning(&source.name);
        }
        let proceed = accept_newer_mind
            || (interactive && confirm(&format!("continue melding {} anyway?", source.name))?);
        if !proceed {
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::IncompatibleVersion {
                source_name: source.name.clone(),
                required,
                running: env!("CARGO_PKG_VERSION").to_string(),
            });
        }
    }

    // Persist the consumer's --root override (STO-17, DSC-51).
    // DSC-52: if --root is given for an authoritative source, print a note.
    let is_authoritative = mindfile.as_ref().is_some_and(|m| m.is_authoritative());

    // MKT-2/MKT-15: when an own-item source-discovery directive is in effect, the
    // plugin-manifest's own-item layer is suppressed. Emit an advisory note so the
    // user is not surprised to find the manifest ignored. Both plugin.json and
    // marketplace.json trigger the note; catalog.rs stays quiet and leaves
    // reporting here. Three distinct cases:
    //   - authoritative `[[items]]`/`[discover]` globs (MKT-2): the manifest is
    //     wholly ignored (its external curator entries are not walked either).
    //   - a mind.toml `[source].roots`/`flat-skills` scan layout (MKT-15): only the
    //     manifest's own-item layer is ignored; convention discovery supplies the
    //     repo's items and any marketplace external entries still compose.
    //   - a consumer `--root`/`--flat-skills` override (MKT-15, DSC-51/DSC-75):
    //     same as the scan-layout case, so the flag is not a silent no-op on a
    //     manifest source.
    // A bare `[discover].sources` list is NOT an own-item directive and prints no
    // note: the manifest still defines the immediate source (MKT-16).
    let declares_scan_layout = mindfile.as_ref().is_some_and(|m| m.declares_scan_layout());
    let consumer_scan_layout = !roots.is_empty() || flat_skills;
    if !out.json && (is_authoritative || declares_scan_layout || consumer_scan_layout) {
        let has_plugin = plugin_manifest::find_plugin_manifest(&dir).is_some();
        let has_marketplace = plugin_manifest::find_marketplace_manifest(&dir).is_some();
        if has_plugin || has_marketplace {
            if is_authoritative {
                println!(
                    "note: {} uses an authoritative mind.toml; its .claude-plugin/ manifest is ignored",
                    source.name
                );
            } else {
                // spec: MKT-15 — the manifest's own-item layer is suppressed; name
                // where the convention layout came from (consumer flag vs mind.toml).
                let via = if declares_scan_layout {
                    "mind.toml [source].roots/flat-skills"
                } else {
                    "--root/--flat-skills"
                };
                println!(
                    "note: {} discovers its own items by convention ({via}); \
                     the .claude-plugin/ manifest's plugin components are ignored",
                    source.name
                );
            }
        }
    }

    if !roots.is_empty() {
        if is_authoritative {
            // spec: DSC-52
            if !out.json {
                println!(
                    "note: {} uses an authoritative mind.toml; --root is ignored",
                    source.name
                );
            }
        } else {
            source.roots = Some(roots);
        }
    } else if apply_curated && curated.roots.is_some() {
        // DSC-61: with no consumer --root override, curator-supplied roots govern
        // convention discovery just like a source's own [source].roots (DSC-50).
        // An open gate means the source is not authoritative.
        source.roots = curated.roots.clone();
    }

    // Persist the consumer's --flat-skills override (STO-44, DSC-75), mirroring
    // the --root handling above.
    // DSC-76: if --flat-skills is given for an authoritative source, print a note
    // (it affects convention discovery only).
    if flat_skills {
        if is_authoritative {
            // spec: DSC-76
            if !out.json {
                println!(
                    "note: {} uses an authoritative mind.toml; --flat-skills is ignored",
                    source.name
                );
            }
        } else {
            source.flat_skills = true;
        }
    } else if apply_curated && curated.flat_skills {
        // spec: DSC-77 / DSC-60 — with no consumer override, curator-supplied
        // flat-skills governs convention discovery, applied only because the
        // gate is open (so the source cannot be authoritative).
        source.flat_skills = true;
    }

    // spec: DSC-84 / STO-55 -- persist the consumer's --add-root roots. They
    // compose with whatever layer is authoritative (manifest, authoritative
    // mind.toml, or convention), so unlike --root there is no ignored/suppressed
    // note to print for a direct consumer `--add-root`. Root validation happens
    // in the scan (InvalidRoot).
    if !add_roots.is_empty() {
        source.add_roots = Some(add_roots);
    } else if apply_curated && curated.add_roots.is_some() {
        // spec: DSC-88 -- unlike a direct consumer `--add-root`, a CURATOR's
        // `add-roots` (from a `[discover].sources` entry) is gated by DSC-60
        // exactly like `roots`/`flat-skills`/hooks: it composes only while the
        // gate is open. Without this gate a
        // curator could reach into a nested source's authoritative export list
        // (DSC-3) and surface everything it deliberately did not export.
        source.add_roots = curated.add_roots.clone();
    }

    // Scan before registering. If the source is rejected here (e.g. the
    // version gate, DSC-40), remove the clone so no orphan is left on disk.
    // Use `!source.is_linked()` (not `!is_local`) so that a pinned-local clone
    // (which is NOT a linked working tree) is also cleaned up on failure (CLI-18,
    // CLI-27).
    let mut items = match catalog::scan(paths, &single(&source)) {
        Ok(items) => items,
        Err(e) => {
            if !source.is_linked() {
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
            // spec: DSC-95 -- sanitize each name before joining (not the
            // composed preview afterward): an unterminated escape in one name
            // would otherwise consume the rest of the joined list.
            let names: Vec<String> = items.iter().map(|it| it.display_key()).collect();
            format!("\n  items would install as: {}", names.join(", "))
        };
        let answer = prompt_line(&format!(
            "{} suggests the prefix '{declared}'.{preview}\n  use it? [Y]es / type a different prefix / [n]o prefix: ",
            source.name
        ))?;
        let chosen = crate::namespace::prefix_choice(&answer);
        // NS-25: a custom prefix typed at the prompt is held to the same rule.
        if let Some(c) = &chosen {
            crate::namespace::validate_prefix(c)?;
        }
        if chosen != source.alias {
            source.alias = chosen;
            items = match catalog::scan(paths, &single(&source)) {
                Ok(items) => items,
                Err(e) => {
                    if !source.is_linked() {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    return Err(e);
                }
            };
        }
    }

    // NS-43: Cross-source skill/rule/tool collision check. Only runs for a
    // top-level `meld` (not for curated nested sources which the caller controls).
    if top_level {
        let manifest = crate::manifest::Manifest::load(paths)?;
        let mut conflicts: Vec<(String, String, String)> = Vec::new();
        for entry in manifest.items.values() {
            // Agents are handled separately by NS-41.
            if entry.kind == crate::error::ItemKind::Agent {
                continue;
            }
            // Skip items from the same source (re-meld / upgrade).
            if entry.source == source.name {
                continue;
            }
            // Collision: same (kind, effective_name) pair from a different source.
            let eff_name = entry.name.as_str();
            if items
                .iter()
                .any(|it| it.kind == entry.kind && it.effective_name() == eff_name)
            {
                conflicts.push((
                    entry.kind.as_str().to_string(),
                    eff_name.to_string(),
                    entry.source.clone(),
                ));
            }
        }
        if !conflicts.is_empty() {
            let suggested = suggested_namespace(&source.url);
            // spec: NS-44 NS-45 — interactive only on a real TTY AND without --yes.
            if crate::hook::is_tty() && !yes {
                // NS-44: interactive TTY path — prompt for a namespace prefix.
                // spec: NS-44
                let answer = prompt_line(&format!(
                    "name collision detected -- the following items conflict with \
                     already-installed items:\n{}\n\
                     enter a namespace prefix [{suggested}] (. to abort): ",
                    format_conflicts_display(&conflicts),
                ))?;
                // spec: NS-44 — parse the collision answer:
                //   empty  → accept the suggested prefix
                //   "."    → abort (SkillCollision, same as non-interactive)
                //   other  → use as custom prefix
                match parse_collision_answer(&answer, &suggested) {
                    CollisionAnswer::Abort => {
                        if !source.is_linked() {
                            let _ = std::fs::remove_dir_all(&dir);
                        }
                        return Err(MindError::SkillCollision {
                            conflicts,
                            suggested,
                        });
                    }
                    CollisionAnswer::Prefix(chosen_str) => {
                        let chosen = if chosen_str.is_empty() {
                            None
                        } else {
                            Some(chosen_str)
                        };
                        if let Some(c) = &chosen {
                            crate::namespace::validate_prefix(c)?;
                        }
                        if chosen != source.alias {
                            source.alias = chosen;
                            items = match catalog::scan(paths, &single(&source)) {
                                Ok(items) => items,
                                Err(e) => {
                                    if !source.is_linked() {
                                        let _ = std::fs::remove_dir_all(&dir);
                                    }
                                    return Err(e);
                                }
                            };
                        }
                    }
                }
            } else {
                // spec: NS-43 NS-45 — non-interactive (no TTY or --yes): hard error.
                if !source.is_linked() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return Err(MindError::SkillCollision {
                    conflicts,
                    suggested,
                });
            }
        }
    }

    warn_forged_namespaces(&items); // spec: DSC-99 advisory
    warn_unguarded_references(&items);
    warn_agent_collisions(paths, &items); // spec: NS-41 advisory
    if !out.json {
        // spec: CLI-205 -- a top-level meld that discovers zero items by
        // convention scanning gets a non-success glyph plus explicit guidance
        // (the convention paths and the escapes), rather than a "0 item(s))"
        // line that reads identically to a legitimate empty source. Suppressed
        // for a nested/curated meld (noise the caller doesn't control), an
        // authoritative mind.toml (roots/--add-root/--flat-skills don't apply
        // there, DSC-52/76), a pure super-source that only curates other
        // sources (zero own items is expected), and a plugin/marketplace
        // manifest source (which has its own guidance path above).
        let curates_other_sources = mindfile
            .as_ref()
            .and_then(|m| m.discover.as_ref())
            .is_some_and(|d| !d.sources.is_empty());
        let has_manifest = plugin_manifest::find_plugin_manifest(&dir).is_some()
            || plugin_manifest::find_marketplace_manifest(&dir).is_some();
        if items.is_empty()
            && top_level
            && !is_authoritative
            && !curates_other_sources
            && !has_manifest
        {
            println!("{} melded {} (0 item(s))", out.warn(), source.name);
            println!(
                "  no items found by convention scanning \
                 (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md, \
                 commands/<name>.md, workflows/<name>.js, \
                 tools/<name>/); if your layout differs, use --root <dir>, \
                 --add-root <dir>, or --flat-skills"
            );
        } else {
            println!(
                "{} melded {} ({} item(s))",
                out.ok(),
                source.name,
                items.len()
            );
        }
        // spec: STO-60 -- when this meld forks a NEW aliased instance (STO-58) of
        // a repo that already has one or more melded instances, say so plainly:
        // the trailing `@<alias>` is otherwise the sole signal that a coexisting
        // instance (a second clone) was registered rather than an existing
        // source's prefix being changed. The registry does not yet contain this
        // source (pushed below), so a prior entry sharing the base identity means
        // a genuine fork.
        if source.as_alias.as_deref().is_some_and(|a| !a.is_empty()) {
            let base = source.base_identity();
            // spec: STO-63 -- name the actual registered instance(s) sharing this
            // base identity, not the bare base itself: when every pre-existing
            // instance is aliased, the bare `host/owner/repo` never appears in the
            // registry and is not a name `unmeld` (or anything else) can resolve.
            let existing: Vec<&str> = st
                .registry
                .sources
                .iter()
                .filter(|s| s.base_identity() == base)
                .map(|s| s.name.as_str())
                .collect();
            if !existing.is_empty() {
                let (subject, verb) = if existing.len() == 1 {
                    (format!("the existing {}", existing[0]), "remains")
                } else {
                    (
                        format!("the existing instances {}", existing.join(", ")),
                        "remain",
                    )
                };
                println!(
                    "note: registered a new instance {}; {subject} {verb}",
                    source.name
                );
            }
        }
    }

    // MKT-4: for a single-plugin source, report any unsupported component kinds
    // (hooks, mcp servers, etc.) that were silently dropped. This is advisory and
    // never a silent drop: the user sees a count so they are not misled into
    // thinking the plugin is fully represented. Only for plugin.json sources
    // (not marketplace catalogs, not convention-discovered sources).
    if !is_authoritative && !out.json && plugin_manifest::find_plugin_manifest(&dir).is_some() {
        let skipped_comps = catalog::plugin_skipped_components(&dir);
        if let Some(summary) = skipped_comps.summary() {
            println!("note: {summary}");
        }
    }

    // spec: HOOK-142 -- resolve the selection the meld applies (asking the
    // PROF-13 picker now) so the install hooks see it. An item link has no
    // profiles (PROF-8).
    let hook_env = profiles.hook_env(
        &source.name,
        mindfile
            .as_ref()
            .filter(|_| source.item_path.is_none())
            .and_then(|m| m.profiles.as_ref()),
    )?;
    // Install hooks (HOOK-50..60): the working tree is now checked out at the
    // resolved pin, so hooks run in the right tree. A fresh meld runs every
    // (as-yet-unrun) install hook.
    //
    // spec: HOOK-180 HOOK-183 -- an interactive top-level meld holds them back
    // and lists them in the plan it asks before installing, so no hook runs
    // before the user has seen what will install. A nested source registered
    // by a curator keeps its own prompt here.
    let defer = top_level && defers_meld_hooks(accept);
    if defer {
        DEFERRED_MELD_HOOKS.with(|d| {
            *d.borrow_mut() = Some(DeferredMeldHooks {
                source: source.name.clone(),
                clone_dir: dir.clone(),
                install_override: install_hook.clone(),
                accept,
                force_rerun: false,
                extra_hooks: curated_hooks.clone(),
                hook_env: hook_env.clone(),
                fresh: true,
            })
        });
    }
    match run_install_hooks(
        &mut source,
        &dir,
        &mindfile,
        &toml_path,
        install_hook.as_deref(),
        accept,
        false,
        // DSC-61: curator-supplied hooks (when applied) run through the same
        // disclosure/safety-prompt/non-TTY-skip path as a source's own hooks.
        curated_hooks,
        &hook_env,
        defer,
    ) {
        Ok(HookOutcome::Proceed) => {}
        Ok(HookOutcome::Abort) => {
            // HOOK-21: aborting installs nothing; the source is not registered.
            // Use `!source.is_linked()` so a pinned-local clone is removed on
            // abort, while a linked working tree is never touched (CLI-27).
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            println!("aborted; nothing installed");
            return Ok(0);
        }
        Err(e) => {
            // HOOK-30/HOOK-53: a hook failure fails the meld; remove the clone.
            // Same guard: remove a pinned-local clone but not the working tree.
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    }

    // MKT-6/MKT-10: record plugin/marketplace provenance and metadata when the
    // source came from a Claude plugin manifest (not suppressed by an authoritative
    // mind.toml). The description from plugin.json fills in only when mind.toml
    // [source].description has not already set it (DSC-32 precedence). Strings
    // from the manifest are passed through strip_ansi to prevent terminal injection
    // (MKT-9/DSC-69).
    // An item-link instance bypasses any plugin manifest (LNK-7), so it never
    // records a manifest origin.
    if !is_authoritative && source.item_path.is_none() {
        if let Some(plugin_path) = plugin_manifest::find_plugin_manifest(&dir) {
            source.origin = Some(ManifestOrigin::ClaudePlugin);
            if let Ok(pm) = plugin_manifest::load_plugin_manifest(&plugin_path) {
                source.plugin_version = pm.version;
                if source.description.is_none() {
                    source.description = pm.description.map(|d| strip_ansi(&d));
                }
            }
        } else if plugin_manifest::find_marketplace_manifest(&dir).is_some() {
            // The marketplace-level origin; per-entry origins are set below (Section C)
            // after each sub-source is recursively melded.
            source.origin = Some(ManifestOrigin::ClaudeMarketplace);
        }
    }

    // Capture the super-source name before moving `source` into the registry,
    // so DSC-63 error messages can reference it.
    let super_source_name = source.name.clone();
    // spec: LNK-8 -- an item-link instance registers no nested sources: the
    // [discover].sources and marketplace-external walks below are skipped.
    let is_link = source.item_path.is_some();
    st.registry.sources.push(source);

    let mut added = 1;
    // DSC-80: count nested entries that failed to register for a non-auth reason,
    // so the curator-empty guard below can distinguish "curator with all nested
    // sources gone" from an ordinary curator that simply has nothing nested.
    let mut nested_clone_failures = 0usize;
    let nested = mindfile
        .as_ref()
        .filter(|_| !is_link) // spec: LNK-8
        .and_then(|m| m.discover.as_ref())
        .map(|d| &d.sources);
    // The install walk already handles direct entries before descendants.
    // Reserve the same identities here so their registration uses the same
    // entries, including any pins recorded by `dump` (DSC-38).
    let top_level_sources: Option<HashSet<String>> = nested.filter(|_| top_level).map(|entries| {
        entries
            .iter()
            .filter_map(|entry| curated_identity(&entry.source, entry.effective_alias()))
            .collect()
    });
    let direct_sources = top_level_sources.as_ref().unwrap_or(direct_sources);
    if let Some(nested) = nested {
        for entry in nested {
            // DSC-64: install = true and a non-empty install-items list are
            // mutually exclusive; error before we register anything.
            entry.validate(&toml_path)?;

            // spec: DSC-93 -- the absolute entry wins. DSC-92 resolves a
            // relative local `source` against the directory the `mind.toml` was
            // read from; for a CLONED curator that directory sits inside mind's
            // own managed sources tree, so `../nested` names a sibling clone dir
            // mind never created. Attempting it is a guaranteed clone failure,
            // and for a curator with no items of its own DSC-80 escalates that
            // into a hard error that aborts the whole meld -- even when the
            // caller (a `dump` reproduction, say) ALSO carries a correct
            // absolute entry for that very source, which the walk simply had not
            // reached yet. Skip it with a warning so the absolute reading gets
            // its turn. A LINKED curator is unaffected: its `mind.toml` is read
            // from the user's own working tree, so its relative entries never
            // resolve into the sources tree, and a genuine typo there still
            // fails exactly as before (DSC-79/DSC-80).
            if let Some(missing) = unresolvable_managed_local_entry(paths, &entry.source) {
                eprintln!(
                    "warning: skipping nested source '{}' curated by '{super_source_name}': it \
                     resolves inside mind's own sources tree ({}) where nothing was cloned, so \
                     it is a relative path that only meant something in the curator's own \
                     working tree; meld that source by an absolute path or a URL instead",
                    missing.display(),
                    paths
                        .all_homes()
                        .iter()
                        .map(|h| h.join("sources"))
                        .find(|s| missing.starts_with(s))
                        .unwrap_or_else(|| paths.sources_dir())
                        .display()
                );
                st.skipped.push(SkippedEntry {
                    source: entry.source.clone(),
                    reason: "unresolvable_local_path".into(),
                });
                continue;
            }
            if !top_level
                && !direct_sources.is_empty()
                && curated_identity(&entry.source, entry.effective_alias())
                    .is_some_and(|name| direct_sources.contains(&name))
            {
                continue;
            }

            // DSC-59 DSC-65: lift this entry's curator-supplied configuration.
            // The pin directive is authoritative (DSC-65). Hooks and roots are
            // gated (DSC-60). All are resolved here against the super-source's
            // mind.toml path; the gate lives in the recursive call.
            // spec: DSC-100 -- an item-link entry's declared kind. Parsed here
            // (a bad word is a MindToml error naming the entry) and threaded
            // through as the instance's explicit kind, NOT through
            // `CuratedConfig`: that carries the DSC-60-gated configuration,
            // while a link's kind is what makes the entry installable at all.
            let entry_kind = entry.item_kind(&toml_path)?;
            let curated = curated_config_for(entry, &toml_path)?;
            // Nested sources from a curated super-source get no consumer pin or
            // root override; the curator config (when applied) supplies them.
            // No consumer roots, add-roots (DSC-88: the curator's is gated),
            // flat-skills, pin, install hook, or --yes for a nested source; it
            // inherits the walk's SSH preference and DSC-116 answer.
            match meld_with_direct_sources(
                st,
                MeldRequest {
                    alias: entry.effective_alias(), // spec: DSC-78 -- prefer `namespace`, fall back to `as`
                    curated: Some(curated),
                    // spec: DSC-100 -- the entry's own `kind =`, for an item-link entry.
                    item_kind: entry_kind,
                    // spec: STO-82 -- provenance for `curate` (CUR-7).
                    curated_by: Some(super_source_name.clone()),
                    // spec: HOOK-142
                    ..MeldRequest::new(&entry.source, &mut MeldProfiles::Curated(entry))
                },
                direct_sources,
            ) {
                Ok(n) => added += n,
                // DSC-68/DSC-69: an auth failure is governed by on-auth-failure
                // when present; without it, it stays a generic git error.
                Err(e) if git::is_auth_failure(&e) => {
                    // spec: STO-58 -- the nested source registers under its
                    // effective alias, so resolve its identity with that alias to
                    // match the DSC-70 "already registered" guard correctly.
                    // spec: CLI-216 -- quiet: the clone this entry names has
                    // already been attempted and failed; re-deriving its name is
                    // answering a question, not deciding a reading.
                    let entry_name = parse_spec_quiet(&entry.source)
                        .map(|mut s| {
                            s.apply_alias(entry.effective_alias());
                            s.name
                        })
                        .unwrap_or_else(|_| entry.source.clone());
                    // spec: DSC-70 -- on-auth-failure only covers the entry's own
                    // clone failure. If the entry is already in the registry, it
                    // cloned successfully and the failure came from a descendant;
                    // propagate it unchanged so it is not misattributed to this entry.
                    if st.registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    let Some(cfg) = &entry.on_auth_failure else {
                        return Err(e);
                    };
                    // spec: DSC-69 -- always warn to stderr regardless of --json mode;
                    // --json controls the outer result format, not warning visibility.
                    for line in auth_failure_lines(&entry_name, cfg) {
                        eprintln!("{line}");
                    }
                    if cfg.action == AuthFailureAction::Skip {
                        st.skipped.push(SkippedEntry {
                            source: entry_name,
                            reason: "auth_failure".into(),
                        });
                        // The source is not registered; its transitive chain is
                        // unreachable and therefore also skipped.
                        continue;
                    }
                    return Err(e);
                }
                // spec: HOOK-172 -- the nested source's own install hook
                // failing (or a Ctrl-C in it) is not a clone failure: it
                // stops the meld, before the source is registered.
                Err(e) if e.is_hook_stop() => return Err(e),
                // spec: DSC-79 -- a non-auth clone failure (network error,
                // not-found, etc.) of a nested entry is skipped with a warning
                // rather than hard-failing the whole meld.
                Err(e) => {
                    // spec: STO-58 -- the nested source registers under its
                    // effective alias, so resolve its identity with that alias to
                    // match the DSC-70 "already registered" guard correctly.
                    // spec: CLI-216 -- quiet, same as the auth-failure arm above.
                    let entry_name = parse_spec_quiet(&entry.source)
                        .map(|mut s| {
                            s.apply_alias(entry.effective_alias());
                            s.name
                        })
                        .unwrap_or_else(|_| entry.source.clone());
                    // spec: DSC-79/DSC-70 -- if the entry is already registered it
                    // cloned fine and the error came from a descendant; propagate
                    // it unchanged rather than misattributing it to this entry.
                    // The primary's own clone failure exits before this loop is
                    // reached (parse/clone of the primary happens in the top-level
                    // meld_recursive call), so this arm only sees nested failures.
                    if st.registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    for line in clone_failure_lines(&entry_name, &e) {
                        eprintln!("{line}");
                    }
                    st.skipped.push(SkippedEntry {
                        source: entry_name,
                        reason: nested_skip_reason(&e),
                    });
                    nested_clone_failures += 1;
                    continue;
                }
            }

            // DSC-63: validate each install_items ref against the nested source's
            // offered bare names. A ref that names a non-existent item is an error
            // at meld, not a silent skip.
            if let Some(refs) = &entry.install_items
                && !refs.is_empty()
                // spec: CLI-216 -- an identity lookup for the DSC-63 validation,
                // not a decision to clone: quiet.
                && let Ok(mut spec) = parse_spec_quiet(&entry.source)
                // spec: STO-58 -- the nested source is registered under its
                // effective alias; resolve against that identity to find it.
                && {
                    spec.apply_alias(entry.effective_alias());
                    true
                }
                && let Some(nested_src) = st.registry.find(&spec.name)
            {
                let nested_items = catalog::scan(paths, &single(nested_src))?;
                for item_ref in refs {
                    // Each ref must be a bare kind:name (DSC-63).
                    let parsed = crate::resolve::parse_item_ref(item_ref).map_err(|_| {
                        MindError::BadReference {
                            item: format!("install-items in '{super_source_name}'"),
                            referent: item_ref.clone(),
                            reason: crate::error::BadRefReason::NoMatch,
                            in_source: spec.name.clone(),
                        }
                    })?;
                    // The ref must name an item the nested source offers
                    // (by bare name, not effective/prefixed name).
                    let found = nested_items.iter().any(|it| {
                        parsed.kind.is_none_or(|k| it.kind == k) && it.name == parsed.name
                    });
                    if !found {
                        return Err(MindError::BadReference {
                            item: format!("install-items in '{super_source_name}'"),
                            referent: item_ref.clone(),
                            reason: crate::error::BadRefReason::NoMatch,
                            in_source: spec.name.clone(),
                        });
                    }
                }
            }
        }
    }

    // MKT-7/MKT-8: marketplace catalog as a curated super-source. When the
    // melded source has a .claude-plugin/marketplace.json (and no authoritative
    // mind.toml), iterate its entries and meld each as a sub-source, reusing the
    // same [discover].sources machinery. This is an additive pass that runs after
    // the mind.toml nested-source loop above.
    if !is_authoritative
        && !is_link // spec: LNK-8
        && let Some(marketplace_path) = plugin_manifest::find_marketplace_manifest(&dir)
    {
        let manifest = plugin_manifest::load_marketplace_manifest(&marketplace_path)?;
        for entry in manifest.into_entries() {
            // MKT-14: in-repo plugins are catalog items of the parent source
            // (handled by catalog::scan_source_at); only external entries
            // need a recursive sub-meld here.
            if matches!(entry.source, plugin_manifest::PluginSource::InRepo { .. }) {
                continue; // spec: MKT-14
            }
            // belt-and-suspenders: into_entries() already validates
            // in-repo paths, so this is a redundant safety check.
            if let plugin_manifest::PluginSource::InRepo { ref path } = entry.source
                && !plugin_manifest::is_safe_manifest_path(path)
            {
                return Err(MindError::MindToml {
                    path: marketplace_path.clone(),
                    msg: format!("marketplace.json: in-repo plugin path {:?} is unsafe", path),
                });
            }
            let repo_spec = marketplace_entry_spec(&entry, &dir);
            let entry_name = entry.name.clone();
            if !direct_sources.is_empty()
                && curated_identity(&repo_spec, Some(entry_name.clone()))
                    .is_some_and(|name| direct_sources.contains(&name))
            {
                continue;
            }
            let entry_version = entry.version;
            let entry_description = entry.description;
            // spec: MKT-9 -- the entry's pin applies like a curator pin
            // (DSC-65), whether or not the plugin ships a mind.toml.
            let entry_pin = match &entry.source {
                plugin_manifest::PluginSource::External { pin, .. } => pin.clone(),
                plugin_manifest::PluginSource::InRepo { .. } => None,
            };
            // Recurse exactly like [discover].sources: cycle-safe
            // via visited, no consumer pin or roots, no install hook.
            match meld_with_direct_sources(
                st,
                MeldRequest {
                    alias: Some(entry_name.clone()),
                    curated: entry_pin.map(|pin| CuratedConfig {
                        pin: Some(pin),
                        ..CuratedConfig::default()
                    }),
                    // spec: STO-82 -- a marketplace catalog curates too (MKT-7).
                    curated_by: Some(super_source_name.clone()),
                    ..MeldRequest::new(&repo_spec, &mut MeldProfiles::None)
                },
                direct_sources,
            ) {
                Ok(n) => {
                    added += n;
                    // MKT-8: marketplace entry fields are authoritative
                    // over the sub-source's own plugin.json. After the
                    // recursive meld, find the just-registered sub-source
                    // by its computed name and overwrite any fields the
                    // entry supplies (entry wins when both supply a value).
                    // spec: CLI-216 -- the sub-meld above already took its
                    // reading; this only re-derives the name to find it.
                    if let Ok(mut sub_spec) = parse_spec_quiet(&repo_spec)
                        && {
                            // spec: STO-58 -- the sub-source was melded under the
                            // entry name as its alias, so its identity is `@<name>`.
                            sub_spec.apply_alias(Some(entry_name.clone()));
                            true
                        }
                        && let Some(sub) = st
                            .registry
                            .sources
                            .iter_mut()
                            .find(|s| s.name == sub_spec.name)
                    {
                        // Always tag as ClaudeMarketplace regardless of
                        // what the sub-source's own plugin.json says.
                        sub.origin = Some(ManifestOrigin::ClaudeMarketplace);
                        if entry_version.is_some() {
                            sub.plugin_version = entry_version;
                        }
                        if entry_description.is_some() {
                            sub.description = entry_description.map(|d| strip_ansi(&d));
                        }
                    }
                }
                Err(e) if git::is_auth_failure(&e) => {
                    // No on_auth_failure is defined for marketplace
                    // entries (no curator config); an auth failure
                    // propagates as a generic error.
                    return Err(e);
                }
                // spec: HOOK-172 -- as in the loop above, a hook stop is not
                // a clone failure.
                Err(e) if e.is_hook_stop() => return Err(e),
                // spec: DSC-79 -- a non-auth clone failure of a marketplace
                // sub-source is skipped with a warning, mirroring the
                // [discover].sources loop above.
                Err(e) => {
                    // spec: STO-58 -- the sub-source registers under the entry
                    // name as its alias, so resolve its identity with that alias.
                    // spec: CLI-216 -- quiet: naming a clone that already failed.
                    let sub_name = parse_spec_quiet(&repo_spec)
                        .map(|mut s| {
                            s.apply_alias(Some(entry_name.clone()));
                            s.name
                        })
                        .unwrap_or_else(|_| entry_name.clone());
                    // spec: DSC-79/DSC-70 -- a failure after the sub-source is
                    // already registered originates from a descendant; propagate.
                    if st.registry.find(&sub_name).is_some() {
                        return Err(e);
                    }
                    for line in clone_failure_lines(&sub_name, &e) {
                        eprintln!("{line}");
                    }
                    st.skipped.push(SkippedEntry {
                        source: sub_name,
                        reason: nested_skip_reason(&e),
                    });
                    nested_clone_failures += 1;
                    continue;
                }
            }
        }
    }

    // spec: DSC-80 -- when the primary source is exclusively a curator (a catalog
    // scan of its own directory yields zero items) and every nested source failed
    // to register, registering a source with no discoverable items is not useful;
    // hard-fail. Only relevant when at least one nested source failed to clone; a
    // primary with its own items, or with any nested source that registered
    // (added > 1), succeeds. The primary/top-level source's own clone failure is a
    // separate hard error handled before the nested loop.
    if nested_clone_failures > 0
        && added == 1
        && let Some(primary) = st.registry.find(&super_source_name)
        // spec: CLI-213 -- a single-source lookup, so it must not degrade: a
        // `LinkedSourceGone` primary would scan as empty here and be reported as
        // "every nested source failed and the curator has nothing of its own",
        // which names the wrong cause.
        && scan_one(paths, primary)?.is_empty()
    {
        return Err(MindError::CuratorAllNestedFailed {
            super_source: super_source_name.to_string(),
        });
    }

    Ok(added)
}

/// Derive a suggested namespace prefix from a source URL or local path.
///
/// Takes the last `/`-separated component and strips any `.git` suffix, so
/// `"https://github.com/foo/bar"` → `"bar"` and `"local/x/y"` → `"y"`.
fn suggested_namespace(url: &str) -> String {
    let base = url.split('/').next_back().unwrap_or(url);
    base.strip_suffix(".git").unwrap_or(base).to_string()
}

/// Format a conflict list for the interactive collision prompt (NS-44).
///
/// Each tuple is `(kind, effective_name, existing_source)`. Produces the same
/// bullet style as the `SkillCollision` error for consistency. Item name and
/// source name are stripped of ANSI escapes so a malicious source cannot inject
/// terminal control sequences into the prompt (spec: NS-44).
fn format_conflicts_display(conflicts: &[(String, String, String)]) -> String {
    conflicts
        .iter()
        .map(|(k, n, s)| {
            let safe_n = strip_ansi(n);
            let safe_s = strip_ansi(s);
            format!("  {k}:{safe_n} (already installed from '{safe_s}')")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The parsed outcome of a user's response to the NS-44 collision prompt.
pub(super) enum CollisionAnswer {
    /// Accept the given prefix (empty string = no prefix, non-empty = custom prefix).
    Prefix(String),
    /// User chose to abort; stop the meld with SkillCollision.
    Abort,
}

/// Parse the user's raw answer to the NS-44 collision prompt (spec: NS-44).
///
/// - Empty (Enter) → accept the pre-populated suggested prefix.
/// - `.` → abort (same outcome as the non-interactive non-TTY path).
/// - Anything else → use as a custom prefix (caller must validate it).
pub(super) fn parse_collision_answer(answer: &str, suggested: &str) -> CollisionAnswer {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        // spec: NS-44 — accepting the suggestion continues under that prefix.
        CollisionAnswer::Prefix(suggested.to_string())
    } else if trimmed == "." {
        // spec: NS-44 — aborting stops meld with a non-zero exit.
        CollisionAnswer::Abort
    } else {
        CollisionAnswer::Prefix(trimmed.to_string())
    }
}

/// Return the display token for a source's manifest origin (MKT-10).
///
/// The token is prepended with a space so it can be concatenated directly into
/// the bracketed metadata string `[commit ns hook origin]`. Returns an empty
/// string when the source has no recorded origin (convention / mind.toml source).
pub(super) fn origin_label(origin: Option<ManifestOrigin>) -> String {
    match origin {
        None => String::new(),
        Some(ManifestOrigin::ClaudePlugin) => " origin:claude-plugin".to_string(),
        Some(ManifestOrigin::ClaudeMarketplace) => " origin:claude-marketplace".to_string(),
    }
}

/// Resolve a [`plugin_manifest::MarketplaceEntry`] to the repo spec string
/// needed by `meld_recursive`.
///
/// - `External { spec }`: the spec is used verbatim (passed to `parse_spec`).
/// - `InRepo { path }`: the path is joined onto `clone_dir` to produce an
///   absolute local path, which `parse_spec` recognises as a local source.
pub(super) fn marketplace_entry_spec(
    entry: &plugin_manifest::MarketplaceEntry,
    clone_dir: &std::path::Path,
) -> String {
    match &entry.source {
        plugin_manifest::PluginSource::External { spec, .. } => spec.clone(),
        plugin_manifest::PluginSource::InRepo { path } => {
            clone_dir.join(path).to_string_lossy().into_owned()
        }
    }
}

/// Build the human-readable lines for an auth failure of a nested source, per
/// DSC-69. The first line is always the standard auth-failure line, with
/// `" (skipping)"` appended under the `"skip"` action. When `message` is set it
/// is the second line, shown immediately after.
pub(super) fn auth_failure_lines(
    entry_name: &str,
    cfg: &crate::mindfile::OnAuthFailure,
) -> Vec<String> {
    // spec: DSC-69
    let is_skip = cfg.action == AuthFailureAction::Skip;
    let safe_name = strip_ansi(entry_name);
    let mut lines = vec![format!(
        "unable to meld source {} due to authentication failure{}",
        safe_name,
        if is_skip { " (skipping)" } else { "" }
    )];
    if let Some(msg) = &cfg.message {
        // spec: DSC-69 -- strip ANSI escape sequences and non-printable bytes so
        // a malicious curator message cannot corrupt the terminal.
        let safe_msg = strip_ansi(msg);
        lines.push(safe_msg);
    }
    lines
}

/// Build the human-readable warning lines for a non-auth clone failure of a
/// nested source, per DSC-79. The entry name and the underlying error are both
/// passed through `strip_ansi` so a malicious remote (via error text) or entry
/// name cannot corrupt the terminal.
pub(super) fn clone_failure_lines(entry_name: &str, err: &MindError) -> Vec<String> {
    // spec: DSC-79
    let safe_name = strip_ansi(entry_name);
    let safe_err = strip_ansi(&err.to_string());
    // spec: HOME-5 HOME-10 -- a home refusal is not a clone failure.
    if matches!(err, MindError::HomeNotAllowed { .. }) {
        return vec![format!("  warning: skipping '{safe_name}': {safe_err}")];
    }
    vec![format!(
        "  warning: skipping '{safe_name}': clone failed ({safe_err}), source unavailable"
    )]
}

/// The `skipped` reason for a nested entry that failed with `err` (DSC-79):
/// `home_not_allowed` for a home refusal (HOME-5, HOME-10), else
/// `clone_failure`.
pub(super) fn nested_skip_reason(err: &MindError) -> String {
    match err {
        MindError::HomeNotAllowed { .. } => "home_not_allowed".into(),
        _ => "clone_failure".into(),
    }
}

/// Handle a top-level (direct, user-initiated) git clone failure for `meld`.
///
/// In human mode (spec: CLI-180):
/// - Prints git's stderr (sanitized via strip_ansi, spec: CLI-186) so the
///   actual cause is immediately visible.
/// - Under `--verbose` also prints the reconstructed command line and the
///   internal store path; without it those details are suppressed.
/// - Prints auth or proxy remediation hints where applicable (CLI-177, CLI-178).
/// - Returns a `MindError::Git` with args reduced to `["clone"]` and stderr set
///   to `"(git output above)"` so the display does not repeat the store path
///   and never shows the literal `<no stderr>` (CLI-185).
///
/// Under `--json` (spec: CLI-184):
/// - Skips all eprintln output (hints and stderr) so only the JSON envelope
///   appears on stdout; git's stderr is preserved in the returned error so the
///   CLI-181 envelope's `message` field carries the actual cause.
fn handle_top_level_clone_err(
    e: MindError,
    ssh_url: Option<String>,
    store_path: &std::path::Path,
    out: crate::render::OutputCtx,
) -> MindError {
    // spec: CLI-184 -- under --json skip all stderr output; preserve the cause
    // so the JSON envelope message is informative.
    if out.json {
        // Keep git's stderr intact; reduce args to hide the internal store path.
        return match e {
            MindError::Git {
                url,
                status,
                stderr,
                ..
            } => MindError::Git {
                url,
                args: vec!["clone".to_string()],
                status,
                stderr,
            },
            other => other,
        };
    }

    // spec: CLI-180, CLI-186 -- lead with git's stderr (sanitized against
    // ANSI/bidi injection from a hostile server).
    if let MindError::Git {
        ref stderr,
        ref args,
        ..
    } = e
    {
        if !stderr.is_empty() {
            eprintln!("{}", strip_ansi(stderr));
        }
        if out.verbose {
            eprintln!("  command: git {}", args.join(" "));
            eprintln!("  store:   {}", store_path.display());
        }
    }
    // spec: CLI-177 -- auth hint for non-local remotes.
    if git::is_auth_failure(&e) {
        if let Some(ref ssh) = ssh_url {
            for line in git::auth_hint_lines(ssh) {
                eprintln!("{line}");
            }
        }
    // spec: CLI-178 -- proxy hint.
    } else if git::is_proxy_failure(&e) {
        for line in git::proxy_hint_lines() {
            eprintln!("{line}");
        }
    }
    // spec: CLI-185 -- set stderr to "(git output above)" so the returned
    // error's Display never shows the misleading literal `<no stderr>` when
    // git output was already streamed to the terminal above.
    match e {
        MindError::Git { url, status, .. } => MindError::Git {
            url,
            args: vec!["clone".to_string()],
            status,
            stderr: "(git output above)".to_string(),
        },
        other => other,
    }
}

/// The registry identity for a repo spec under a consumer alias (STO-58):
/// `host/owner/repo[#path]` plus a trailing `@<alias>`. This is the key
/// `is_melded`/`remeld` look up, so `--as <prefix>` selects (or creates) the
/// `host/owner/repo@<prefix>` instance rather than the bare repo. `Some("")`
/// (the `--as ''` no-prefix override) resolves to the bare identity, so it names
/// the same instance as `None`.
pub fn instance_name(repo: &str, alias: Option<&str>) -> Result<String> {
    // spec: CLI-216 -- naming an instance answers a question ("which instance
    // does this spec select?"); it never clones. `meld_recursive`'s parse is the
    // one that decides a reading, and it carries the CLI-215 note. Without this,
    // the dispatcher's two identity calls (`instance_name` for the post-meld
    // steps and `is_melded` for the routing) printed the note twice more.
    let mut source = parse_spec_quiet(repo)?;
    source.apply_alias(alias.map(str::to_string));
    Ok(source.name)
}

/// True when the repo spec, under the given consumer alias, resolves to an
/// already-registered source instance (STO-58).
pub fn is_melded(paths: &Paths, repo: &str, alias: Option<&str>) -> Result<bool> {
    let name = instance_name(repo, alias)?;
    Ok(Registry::load(paths)?.find(&name).is_some())
}

/// The DSC-121 note for `--full-history` on a linked local source.
fn full_history_linked_note(name: &str) -> String {
    format!(
        "note: {} is your own working tree, so --full-history does not apply",
        strip_ansi(name)
    )
}

/// `meld --full-history` on an already-melded source (DSC-121): record the
/// setting and deepen the existing clone now, before the re-meld's hook and
/// install passes run. A linked local source is the user's own working tree,
/// which `mind` never fetches into, so it is left alone with a note.
// spec: DSC-121
fn enable_full_history(paths: &Paths, name: &str) -> Result<()> {
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == name) else {
        return Ok(());
    };
    if source.is_linked() {
        eprintln!("{}", full_history_linked_note(&source.name));
        return Ok(());
    }
    let dir = clone_dir_checked(paths, source)?;
    git::deepen(&source.url, &dir)?;
    if !source.full_history {
        source.full_history = true;
        registry.save(paths)?;
    }
    Ok(())
}

/// Re-melding an already-melded source (CLI-12). It does not re-clone or
/// re-register: it ensures the source's items are installed, installing any that
/// are missing just as a fresh meld does (CLI-23), and otherwise (or with
/// `--link-only`) prints a status of the source's items and the commit each is
/// installed at.
///
/// `pin` is the caller's `--pin` request (`PinRequest::None` if the flag was
/// not given): unlike the other discovery flags, a re-meld DOES honor it
/// (CLI-209) by re-pinning the source before the hook-rerun/install passes
/// below, so they act on the possibly-new commit.
///
/// `ignored_flags` names the discovery CLI flags the caller passed on this
/// invocation (`--root`, `--add-root`, `--flat-skills`, `--install-hook`) that
/// a re-meld does not apply (CLI-206): they change what gets discovered, which
/// only happens at the meld that first registers a source. When non-empty, a
/// one-line note lists them so they are not silently dropped.
// spec: CLI-12, CLI-206, CLI-209
#[allow(clippy::too_many_arguments)]
pub fn remeld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    link_only: bool,
    flow: InstallFlow,
    recursive: bool,
    ignored_flags: &[&str],
    pin: PinRequest,
    learn_patterns: &[String],
    profile_flags: Option<&crate::profile::Selection>,
) -> Result<()> {
    // `yes` rides inside `flow` for the install calls; remeld itself only needs
    // the clobber (hook force-rerun) and the accepted hook events.
    let InstallFlow {
        clobber, accept, ..
    } = flow;
    let out = crate::render::ctx();
    // spec: STO-58/CLI-12 -- a re-meld targets the specific instance named by the
    // repo spec plus the consumer alias. `is_melded` used the same identity to
    // route here, so the alias matches this instance and is not a re-prefixing
    // (CLI-13): `--as` denoting a different alias would have been a fresh meld.
    let source_name = instance_name(repo, alias.as_deref())?;
    if !out.json {
        println!("{} {source_name} is already melded", out.bullet());
        // spec: CLI-206 -- a re-meld does not re-clone or re-register, so a
        // discovery flag on this invocation is a silent no-op without this
        // note (mirrors the CLI-203 note pattern for `learn <url> --pin`).
        if !ignored_flags.is_empty() {
            let flags = ignored_flags.join(", ");
            let plural = ignored_flags.len() != 1;
            // spec: CLI-225 -- `source_name` is source-influenced (an
            // instance name derived from the repo spec plus the consumer
            // alias) and lands in a pasteable `mind unmeld <name>` remedy, so
            // it is shell-quoted before printing (same rule as the
            // `HooksNotRun`/`AmbiguousHookTarget` remedies in error.rs).
            let quoted_source = crate::error::shell_quote(&source_name);
            println!(
                "note: {flags} ignored; {source_name} is already melded (re-melding \
                 does not change a source's discovery configuration). To apply \
                 {}, run `mind unmeld {quoted_source}` then `mind meld` again with {}.",
                if plural { "them" } else { "it" },
                if plural { "the flags" } else { "the flag" }
            );
        }
    }

    // spec: CLI-209 -- honor `--pin` on a re-meld by re-pinning the source,
    // before the hook-rerun/install passes below so they see the (possibly
    // new) commit.
    if pin != PinRequest::None {
        repin_source(paths, &source_name, pin)?;
    }

    // spec: PROF-16 -- resolved before the hooks so they see it (HOOK-142).
    let remeld_selection = if link_only {
        None
    } else {
        remeld_profile_selection(paths, &source_name, profile_flags)?
    };

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
            let hook_env = meld_hook_env(
                mindfile.as_ref().and_then(|m| m.profiles.as_ref()),
                remeld_selection.as_ref().map(|(sel, _)| sel),
                // spec: HOOK-142 PROF-40 -- a replacing selection is the
                // whole set the hooks see, not an addition to the recorded one.
                match &remeld_selection {
                    Some((_, true)) => None,
                    _ => registry.sources[idx].profiles.as_ref(),
                },
            );
            // spec: HOOK-180 HOOK-183 -- as for a fresh meld, an interactive
            // re-meld lists the pending hooks in its plan instead of asking
            // here.
            let defer = defers_meld_hooks(accept);
            if defer {
                DEFERRED_MELD_HOOKS.with(|d| {
                    *d.borrow_mut() = Some(DeferredMeldHooks {
                        source: source_name.clone(),
                        clone_dir: clone_dir.clone(),
                        install_override: None,
                        accept,
                        force_rerun,
                        extra_hooks: Vec::new(),
                        hook_env: hook_env.clone(),
                        fresh: false,
                    })
                });
            }
            match run_install_hooks(
                &mut registry.sources[idx],
                &clone_dir,
                &mindfile,
                &toml_path,
                None,
                accept,
                force_rerun,
                Vec::new(),
                &hook_env,
                defer,
            ) {
                Ok(HookOutcome::Proceed) => registry.save(paths)?,
                Ok(HookOutcome::Abort) => {
                    registry.save(paths)?; // persist any hook that did run
                    if !out.json {
                        println!("aborted; source left in place");
                    }
                    return Ok(());
                }
                Err(e) => {
                    // spec: LIFE-48 -- H3: persist any earlier hook's ran_at
                    // update in this pass before propagating a later hook's
                    // failure; otherwise a hook that already succeeded is
                    // silently re-offered on the next meld/upgrade (mirrors
                    // HOOK-53's own "leave the source melded" intent).
                    registry.save(paths)?;
                    return Err(e); // a hook failed; leave the source melded
                }
            }
        }
    }

    if let Some((selection, replace)) = remeld_selection {
        // spec: PROF-16 PROF-40 -- a re-meld honors the profile flags, which
        // replace a tracked source's recorded set; the curated chain is
        // walked as for install-all, but not under `--learn` (CLI-236).
        let req = ProfileInstall {
            source_name: &source_name,
            selection: &selection,
            learn_patterns,
            only_missing_from_snapshot: false,
            replace,
        };
        let outcome = install_source_profiles(paths, &req, flow)?;
        let mut installed = outcome.installed_keys;
        if learn_patterns.is_empty() {
            installed.extend(install_curated_sources(
                paths,
                &source_name,
                recursive,
                flow,
            )?);
        } else {
            note_learn_ignores_recursive(recursive);
        }
        if !out.json {
            return source_status(paths, &source_name);
        }
        let mut result = MutationResult::new("meld", &source_name, "already-melded");
        result.installed = installed.iter().map(|k| strip_ansi(k)).collect();
        if outcome.pending > 0 {
            result.pending_items = Some(outcome.pending);
        }
        result.profiles = Some(outcome.enabled);
        // spec: PROF-40 -- the json twin of the leftovers note.
        if replace && outcome.applied {
            result.profile_leftovers = source_profile_leftovers(paths, &source_name)
                .0
                .iter()
                .map(|k| strip_ansi(k))
                .collect();
        }
        return print_json(&result);
    }
    if !link_only {
        // spec: CLI-217 CLI-156 -- the `--json` re-meld installs through the
        // SILENT helpers `meld`'s fresh branch uses, then answers with ONE
        // object of its own. Routed through `install_source_items` instead, this
        // branch emitted `learn`'s result object (an action the caller never
        // invoked) and either returned before its own object or printed a second
        // one after it.
        if out.json {
            // spec: CLI-236 -- a re-meld honors `--learn` too: the named subset
            // installs instead of the whole set, and the curated chain
            // (DSC-54/55/58) is left alone.
            let outcome = if learn_patterns.is_empty() {
                let mut outcome = install_source_items(paths, &source_name, flow)?;
                outcome.installed.extend(install_curated_sources(
                    paths,
                    &source_name,
                    recursive,
                    flow,
                )?);
                outcome
            } else {
                install_source_items_matching(paths, &source_name, learn_patterns, flow)?
            };
            let mut result = MutationResult::new("meld", &source_name, "already-melded");
            // spec: DSC-95 -- `installed` rides straight off `learn_collecting`'s
            // raw keys, each embedding a source-controlled bare name; sanitize
            // every one before it lands in the `--json` envelope, the same way
            // `emit_meld_json_result` and `learn()` do for their own.
            result.installed = outcome.installed.iter().map(|k| strip_ansi(k)).collect();
            if outcome.pending > 0 {
                result.pending_items = Some(outcome.pending);
            }
            return print_json(&result);
        }
        // spec: CLI-236 -- the named subset only; a pattern matching nothing is
        // an error, and the curated chain is not walked. The CLI-12 status
        // report still runs afterwards, as it does for any other re-meld.
        if !learn_patterns.is_empty() {
            note_learn_ignores_recursive(recursive);
            install_source_items_matching(paths, &source_name, learn_patterns, flow)?;
            return source_status(paths, &source_name);
        }
        let item_ref = format!("{source_name}#*");
        let to_install = match learn_preview(paths, &item_ref) {
            Ok(plan) => plan.install_count,
            Err(MindError::ItemNotFound { .. }) => 0,
            Err(e) => return Err(e),
        };
        if to_install > 0 {
            install_source_items(paths, &source_name, flow)?;
            // Install the curated chain: every nested source with `--recursive`
            // (DSC-55), or just the curator's `install = true` entries (DSC-58).
            // The nested sources are already registered, so nothing re-registers.
            install_curated_sources(paths, &source_name, recursive, flow)?;
            return Ok(());
        }
        // A pure super-source has no own items; still install the curated chain
        // (all of it with --recursive, else the `install = true` entries).
        install_curated_sources(paths, &source_name, recursive, flow)?;
        if recursive {
            return Ok(());
        }
    }
    // Only `--register-only` (link_only) reaches this under `--json`; the
    // install branch above answers with its own object.
    if out.json {
        return print_json(&MutationResult::new("meld", &source_name, "already-melded"));
    }
    source_status(paths, &source_name)
}

/// Install the items of the registered sources a super-source curates, walking
/// its transitive `[discover].sources` chain. The whole chain is always traversed
/// (so a deeper `install = true` or `install-items` is reached), but a given
/// nested source's items are offered for install only when:
///
/// - `all` is set (`meld --recursive`, DSC-55): install everything, or
/// - the entry has `install-items = [...]` (DSC-62): install exactly that subset, or
/// - the entry has `install = true` (DSC-58): install all of the nested source.
///
/// When `install-items` is present it governs; when absent `install` governs.
/// Reads each source's clone `mind.toml`; cycle-safe via a visited set; only
/// touches registered sources.
///
/// Under `--json` every install is silent and the installed keys are returned,
/// so the meld dispatcher folds them into ONE object (CLI-156); in text mode
/// the list is empty.
pub(crate) fn install_curated_sources(
    paths: &Paths,
    super_name: &str,
    all: bool,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    // spec: HOOK-183 -- an aborted plan stops the meld before the chain.
    if meld_aborted() {
        return Ok(Vec::new());
    }
    let registry = Registry::load(paths)?;
    let mut visited: HashSet<String> = HashSet::from([super_name.to_string()]);
    let mut queue: Vec<String> = vec![super_name.to_string()];
    let mut all_keys: Vec<String> = Vec::new();
    while let Some(name) = queue.pop() {
        let Some(source) = registry.find(&name) else {
            continue;
        };
        // spec: LNK-8 -- an item-link instance curates nothing.
        if source.item_path.is_some() {
            continue;
        }
        let nested = MindToml::load(&source.clone_dir(paths))?
            .and_then(|m| m.discover)
            .map(|d| d.sources)
            .unwrap_or_default();
        for ns in nested {
            // spec: CLI-216 -- the install walk only resolves already-registered
            // identities; it clones nothing, so it must not repeat the CLI-215
            // note the melding parse already printed for this same entry.
            let Ok(mut spec) = parse_spec_quiet(&ns.source) else {
                continue;
            };
            // spec: STO-58 -- the nested source was melded under this entry's
            // effective alias (DSC-78), so its registered identity carries the
            // `@<alias>` suffix; resolve against that, not the bare name.
            spec.apply_alias(ns.effective_alias());
            if !visited.insert(spec.name.clone()) {
                continue; // already seen (cycle guard / diamond dedup)
            }
            if registry.find(&spec.name).is_some() {
                // Traverse every nested source, but install according to the
                // directive in effect (DSC-55 > DSC-62 > DSC-58):
                // - `all` (--recursive): install everything
                // - `install_items` Some(list): install exactly that subset
                //   (empty list = install nothing, like install = false)
                // - `install = true`: install all of the nested source
                let keys = if all {
                    install_source_items(paths, &spec.name, flow)?.installed
                } else if let Some(selection) =
                    curated_profile_selection(paths, &registry, &ns, &spec.name, &name)?
                {
                    // spec: PROF-26 -- the entry's profiles / default-profile
                    // apply like the flags; `install-items` joins the batch.
                    let patterns = curated_install_item_patterns(
                        paths,
                        &registry,
                        &spec.name,
                        ns.install_items.as_ref(),
                    )?;
                    install_source_profiles(
                        paths,
                        &ProfileInstall {
                            source_name: &spec.name,
                            selection: &selection,
                            learn_patterns: &patterns,
                            only_missing_from_snapshot: false,
                            replace: false,
                        },
                        flow,
                    )?
                    .installed_keys
                } else if let Some(refs) = &ns.install_items {
                    // DSC-62: install_items governs; refs is the subset to offer.
                    install_source_items_subset(paths, &spec.name, refs, flow)?.installed
                } else if ns.install {
                    // DSC-58: install all of this nested source.
                    install_source_items(paths, &spec.name, flow)?.installed
                } else {
                    Vec::new()
                };
                all_keys.extend(keys);
                queue.push(spec.name);
            }
        }
        // MKT-7: a marketplace catalog is a curated super-source too. Its in-repo
        // plugins are offered for install on meld like the catalog's own items
        // (CLI-23); its external plugins are register-only unless `all`
        // (--recursive, DSC-55), mirroring the DSC-54 default for nested sources.
        for (spec, in_repo) in marketplace_subsources(paths, source)? {
            if !visited.insert(spec.name.clone()) {
                continue;
            }
            if registry.find(&spec.name).is_some() {
                if all || in_repo {
                    all_keys.extend(install_source_items(paths, &spec.name, flow)?.installed);
                }
                queue.push(spec.name);
            }
        }
    }
    Ok(all_keys)
}

/// Resolve the marketplace sub-sources a source curates (MKT-7): each
/// `.claude-plugin/marketplace.json` entry mapped to `(parsed source spec,
/// is_in_repo)`. Empty when the source has no marketplace manifest or an
/// authoritative `mind.toml` suppresses it (MKT-2). In-repo entries install by
/// default; external entries are register-only unless `--recursive`.
pub(crate) fn marketplace_subsources(
    paths: &Paths,
    source: &crate::source::Source,
) -> Result<Vec<(crate::source::Source, bool)>> {
    let clone = source.clone_dir(paths);
    if MindToml::load(&clone)?.is_some_and(|m| m.is_authoritative()) {
        return Ok(Vec::new());
    }
    let Some(mp) = plugin_manifest::find_marketplace_manifest(&clone) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in plugin_manifest::load_marketplace_manifest(&mp)?.into_entries() {
        let in_repo = matches!(entry.source, plugin_manifest::PluginSource::InRepo { .. });
        // spec: CLI-216 -- resolving a marketplace entry to the identity it was
        // melded under; the meld already happened, so this parse is quiet.
        if let Ok(mut spec) = parse_spec_quiet(&marketplace_entry_spec(&entry, &clone)) {
            // spec: STO-58/MKT-8 -- an external entry was melded under the entry
            // name as its alias, so its registered identity carries `@<name>`.
            spec.apply_alias(Some(entry.name.clone()));
            out.push((spec, in_repo));
        }
    }
    Ok(out)
}

/// Build and emit the single meld JSON result (CLI-153, CLI-156).
///
/// Called by the dispatcher in `main.rs` after both the registration step
/// (`meld()`) and the post-meld install step (`install_source_items`)
/// have completed, so ONE object covers both outcomes.
///
/// `installed` contains the effective keys installed in this call.
/// `pending` is non-zero when `--yes` was absent and items remain to install.
// spec: CLI-153 CLI-156
///
/// `profiles` is the enabled profile set when the install went through
/// profiles (PROF-18), `None` otherwise.
pub(crate) fn emit_meld_json_result(
    summary: MeldSummary,
    installed: Vec<String>,
    pending: usize,
    profiles: Option<Vec<String>>,
) -> Result<()> {
    // spec: CLI-153 -- a top-level meld registers its source unless the user
    // aborted at an install-hook prompt (HOOK-21), so none added is an abort.
    let outcome = if summary.added == 0 {
        "aborted"
    } else {
        "melded"
    };
    let mut result = MutationResult::new("meld", &summary.source_name, outcome);
    result.profiles = profiles;
    result.count = Some(summary.added);
    result.skipped = summary.skipped;
    // spec: DSC-95 -- `installed` rides straight off `learn_collecting`'s raw
    // (unsanitized) keys; sanitize each one before it lands in the `--json`
    // envelope, mirroring `learn()`'s own `result.installed` assignment.
    result.installed = installed.iter().map(|k| strip_ansi(k)).collect();
    if pending > 0 {
        result.pending_items = Some(pending);
    }
    print_json(&result)
}

/// Print every item the source offers with its install state and the source
/// commit it was installed from, noting items whose commit lags the source
/// (CLI-12). Items are matched to the manifest by stable identity (source, kind,
/// bare name), so a prefix change does not lose them.
pub(super) fn source_status(paths: &Paths, source_name: &str) -> Result<()> {
    let out = crate::render::ctx();
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Err(MindError::SourceNotFound {
            name: source_name.to_string(),
        });
    };
    // spec: CLI-213 -- `meld`'s already-melded branch reaches this, and meld's own
    // scan hard-fails on any scan error. `catalog::scan` would degrade a
    // `LinkedSourceGone` source into `Ok(vec![])` and print it as a healthy source
    // with "0 item(s)", contradicting the stderr warning next to it.
    let items = scan_one(paths, source)?;
    let manifest = Manifest::load(paths)?;

    let head = source
        .commit
        .as_deref()
        .map(short)
        .unwrap_or_else(|| "?".to_string());
    println!(
        "{} {source_name}: {} item(s) (source @ {head})",
        out.bullet(),
        items.len()
    );
    for it in &items {
        let installed = manifest
            .items
            .values()
            .find(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name);
        match installed {
            Some(m) => {
                // CLI-75 / LIFE-11: an item is outdated exactly when `upgrade`
                // would act on it -- its source-content hash changed, or its
                // effective name changed (a namespace/prefix rename). A commit
                // advance that did not touch the item's content or name does NOT
                // mark it outdated; `upgrade` would report nothing pending for it.
                //
                // CLI-75: `upgrade` aborts the whole run via `?` when it cannot
                // hash the source content. This best-effort listing marker cannot
                // abort -- it must still print the other rows -- so it errs toward
                // flagging: a hash-computation error counts as drift (outdated)
                // rather than being silently read as "up to date". The same
                // hash-error-counts-as-lag rule is applied at all four marker
                // sites for consistency.
                let hash_lag = it.content_hash().map_or(true, |h| h != m.hash);
                let rename_lag = it.effective_name() != m.name;
                let stale = hash_lag || rename_lag;
                let lag = if stale {
                    out.yellow(" (outdated; run `mind upgrade`)")
                } else {
                    String::new()
                };
                // Stale installs use the ↑ marker, distinct from ✓ for current.
                let marker = if stale { out.stale() } else { out.ok() };
                // spec: DSC-95
                println!(
                    "  {} {}  installed @ {}{}",
                    marker,
                    it.display_key(),
                    out.green(&short(&m.commit)),
                    lag
                );
            }
            // spec: CLI-225 -- `it.key()` is a source-influenced `kind:name`
            // identity and lands in a pasteable `mind learn` remedy, so it is
            // shell-quoted before printing rather than framed in bare single
            // quotes.
            // spec: DSC-95 -- sanitize before shell-quoting too, so a
            // control/ANSI byte cannot ride the pasteable remedy either.
            None => println!(
                "  {} {}  not installed (run `mind learn {}`)",
                out.available(),
                it.display_key(),
                crate::error::shell_quote(&it.display_key())
            ),
        }
    }
    Ok(())
}

/// Set (or clear) the namespace prefix for a source, enforcing the mutability
/// lock (NS-30/CLI-161): the prefix may only change while no items from the
/// source are installed. If items are installed and the requested prefix differs
/// from the current one, returns `NamespaceLocked` listing those items. When the
/// prefix is unchanged nothing is written. Exported for the TUI source-details
/// dialog (TUI-53).
///
/// This changes the source's effective display prefix (`alias`), not its identity
/// (STO-58): the identity alias (`as_alias`) is fixed at meld, so this never
/// renames the source or relocates its clone.
pub fn set_source_namespace(
    paths: &Paths,
    source_name: &str,
    new_alias: Option<String>,
) -> Result<()> {
    // NS-25: validate the prefix (validate_prefix accepts None/empty).
    if let Some(ref a) = new_alias {
        crate::namespace::validate_prefix(a)?;
    }
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) else {
        return Ok(()); // source not found; nothing to change
    };
    let current = source.alias.clone().unwrap_or_default();
    let requested = new_alias.clone().unwrap_or_default();
    if current == requested {
        return Ok(()); // no change; nothing to do
    }
    // NS-30/CLI-161: check whether any of this source's items are installed.
    // DSC-95: `MindError::NamespaceLocked`'s `#[error(...)]` Display joins
    // `items` verbatim into the rejection message with no sanitizing step of
    // its own, so each key must already be display-safe here.
    let manifest = Manifest::load(paths)?;
    let installed_names: Vec<String> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .map(|it| it.key().display())
        .collect();
    if !installed_names.is_empty() {
        return Err(MindError::NamespaceLocked {
            src_name: source_name.to_string(),
            items: installed_names,
        });
    }
    source.alias = new_alias;
    registry.save(paths)
}

/// Register one `[discover].sources` entry of `curator`, exactly as the meld
/// nested loop and the DSC-57 re-walk do (curate.md CUR-3).
///
/// Register-only: the entry's declared items are installed by the caller's
/// install pass (CUR-4), so this is the registration half alone. Returns how
/// many sources it added (0 when the entry was already registered, or was
/// skipped).
#[allow(clippy::too_many_arguments)]
pub(crate) fn meld_curated_entry(
    paths: &Paths,
    registry: &mut Registry,
    curator: &str,
    toml_path: &std::path::Path,
    entry: &crate::mindfile::NestedSource,
    policy: Option<&Policy>,
    prefer_ssh: bool,
    accept: crate::hook::Accept,
    skipped: &mut Vec<SkippedEntry>,
) -> Result<usize> {
    entry.validate(toml_path)?; // spec: DSC-64
    let curated = curated_config_for(entry, toml_path)?;
    let item_kind = entry.item_kind(toml_path)?; // spec: DSC-100
    let mut visited: HashSet<String> = registry
        .sources
        .iter()
        .map(|s| format!("{}|{}", s.name, s.url))
        .collect();
    // The curator's directive supplies the pin (DSC-65); there is no consumer
    // install hook or --full-history (DSC-121).
    meld_recursive(
        &mut MeldState {
            paths,
            registry,
            visited: &mut visited,
            policy,
            accept,
            prefer_ssh,
            skipped,
            accept_newer_mind: false, // spec: DSC-116 -- no one to ask; refuse
        },
        MeldRequest {
            alias: entry.effective_alias(), // spec: DSC-78
            // Non-interactive: `curate` has already taken the user's decision.
            yes: true,
            curated: Some(curated),
            item_kind,
            curated_by: Some(curator.to_string()), // spec: STO-82
            // spec: HOOK-142
            ..MeldRequest::new(&entry.source, &mut MeldProfiles::Curated(entry))
        },
    )
}

/// The `meld` verb end to end: register (or re-meld) the source, then run
/// the install offer the flags select (CLI-23, CLI-236, PROF-9..13). Shared
/// by the CLI dispatch and the probe TUI's meld input (TUI-77), so a TUI
/// meld takes exactly the flags and flow `mind meld` does.
pub fn meld_verb(paths: &Paths, args: crate::cli::MeldArgs, yes: bool, json: bool) -> Result<()> {
    let crate::cli::MeldArgs {
        repo,
        alias,
        roots,
        add_roots,
        flat_skills,
        full_history,
        pin,
        follow_branch,
        pin_tag,
        pin_ref,
        install_hook,
        accept,
        register_only,
        kind,
        learn_patterns,
        profiles,
        no_default_profile,
        all_profiles,
        recursive,
        force,
        overwrite,
        local,
        auto_update,
        home,
        lobe,
    } = args;
    let accept = accept.accept();
    // spec: HOME-4 -- `--home` must name an allowed home; it is checked before
    // anything is cloned, and governs every source this meld registers.
    let _home_scope = match home.as_deref() {
        Some(h) => Some(crate::paths::MeldHomeScope::set(crate::paths::MeldHome {
            home: paths.allowed_home(h, "--home")?,
            explicit: true,
        })),
        None => None,
    };
    // spec: HOOK-180 HOOK-183 -- an interactive meld holds its source install
    // hooks back for the one plan it asks before installing.
    let _plan = MeldPlanScope::enter(!json && crate::hook::is_tty());
    // spec: AUP-8 -- a bad level is refused before the clone.
    let grant = auto_update
        .as_deref()
        .map(parse_auto_update_grant)
        .transpose()?;
    // spec: HARN-20/HARN-21 -- `--local` restricts the install fan-out to
    // the registered project lobe containing the cwd; without it, plain
    // meld/learn inside such a lobe gets a one-line note that --local is
    // available. The guard lives for the whole arm so every install path
    // (fresh meld, re-meld, curated chain) is scoped.
    let _local_guard = apply_local_scope(paths, local)?;
    // spec: CLI-17, CLI-200..202 -- fold the pin flag (and deprecated
    // aliases) into one request; more than one is a structured
    // ConflictingPin error rather than a clap usage string.
    let pin = parse_pin_flags(pin, follow_branch, pin_tag, pin_ref)?;
    // spec: CLI-239 -- `--kind` declares a file link's item kind; a bad
    // value, or one passed with a plain repo spec, is refused before the
    // clone. Resolved after `repo` is defaulted below.
    // spec: CLI-236 -- reject an unusable `--learn` value before the
    // clone, so a typo does not leave a registered source behind.
    validate_learn_patterns(&learn_patterns)?;
    // spec: PROF-14 -- a malformed `--profile` name is refused before
    // the clone, for the same reason.
    let profile_names = crate::profile::parse_cli_names(&profiles)?;
    let profile_flags =
        crate::profile::flags_given(&profile_names, no_default_profile, all_profiles);
    let profile_selection = crate::profile::Selection {
        include_default: !no_default_profile,
        named: profile_names,
        all: all_profiles,
    };
    // CLI-25: no repo argument (or an explicit `.`/`./`) melds the
    // current directory. Resolve it to an absolute path so `parse_spec`
    // derives a sensible `local/<parent>/<dir>` identity.
    let repo = match repo.as_deref() {
        None | Some(".") | Some("./") => std::env::current_dir()
            .map_err(|e| crate::error::MindError::io(".", e))?
            .to_string_lossy()
            .into_owned(),
        Some(r) => r.to_string(),
    };
    let item_kind = parse_link_kind(kind, &repo)?;
    // spec: PROF-8 -- an item link is a single item: no profiles.
    if is_item_link_spec(&repo) {
        crate::profile::reject_on_item_link(&profile_flags)?;
    }
    // CLI-35: `--force` overwrites a conflicting target; otherwise a
    // conflict prompts on a TTY (Clobber::Prompt). spec: CLI-246 --
    // `--overwrite` accepts every conflict but, unlike `--force`, leaves the
    // HOOK-60 re-offer to the pending hooks.
    let clobber = if force {
        Clobber::Force
    } else if overwrite {
        Clobber::Overwrite
    } else {
        Clobber::Prompt
    };
    let flow = InstallFlow {
        yes,
        clobber,
        accept,
    };
    // spec: STO-58 -- the alias is part of the source identity, so the
    // instance a re-meld/install targets is `host/owner/repo@<alias>`.
    // Computed before `alias` is moved into meld/remeld.
    let source_ident = instance_name(&repo, alias.as_deref()).unwrap_or_else(|_| repo.clone());
    // CLI-12: re-melding an already-melded source is not an error; it
    // ensures the items are installed, else reports their status.
    let already_melded = is_melded(paths, &repo, alias.as_deref())?;
    // spec: LOBE-2 LOBE-7 -- a `--lobe` that is not allowed is refused before
    // the clone; recorded now so the meld below already honors it.
    // A relative one is taken against the `--home` given with it.
    let mut _lobe_scope = None;
    if let Some(l) = lobe.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        let lobe_home = if home.is_some() {
            crate::paths::meld_home()
                .home
                .unwrap_or_else(|| paths.mind_home.clone())
        } else {
            paths.home_of(&source_ident)
        };
        paths.resolve_lobe_in(&lobe_home, l, "--lobe")?;
        if !already_melded {
            _lobe_scope = Some(crate::paths::PendingLobeScope::set(&source_ident, l));
        }
    }
    if already_melded {
        // spec: HOME-8 -- `--home` on a melded source moves it, and the
        // sources and items that follow it, into that home.
        if home.is_some() {
            let requested = crate::paths::meld_home().home;
            move_to_home(paths, &source_ident, requested.as_deref(), yes, accept)?;
        }
        // spec: LOBE-7 -- `--lobe` on a melded source moves its links.
        if let Some(l) = lobe.as_deref() {
            set_lobe(paths, &source_ident, l, yes, accept)?;
        }
        // spec: CLI-206 -- name every discovery flag this invocation
        // carries; remeld() cannot apply any of them (it never
        // re-clones or re-registers) and reports so explicitly.
        let mut ignored_flags: Vec<&str> = Vec::new();
        if !roots.is_empty() {
            ignored_flags.push("--root");
        }
        if !add_roots.is_empty() {
            ignored_flags.push("--add-root");
        }
        if flat_skills {
            ignored_flags.push("--flat-skills");
        }
        if install_hook.is_some() {
            ignored_flags.push("--install-hook");
        }
        if item_kind.is_some() {
            ignored_flags.push("--kind");
        }
        // spec: DSC-121 -- `--full-history` is honored on a re-meld too.
        if full_history {
            enable_full_history(paths, &source_ident)?;
        }
        // spec: CLI-209 -- unlike the discovery flags above, `--pin`
        // IS honored on a re-meld: it re-pins the source instead of
        // being a silent no-op.
        remeld(
            paths,
            &repo,
            alias,
            register_only,
            flow,
            recursive,
            &ignored_flags,
            pin,
            &learn_patterns,
            // spec: PROF-16
            (!profile_flags.is_empty()).then_some(&profile_selection),
        )?;
    } else {
        // spec: HOOK-142 -- the source's install hooks see the selection.
        let mut meld_profiles = if register_only {
            MeldProfiles::None
        } else {
            MeldProfiles::Consumer {
                selection: profile_selection.clone(),
                flags_given: !profile_flags.is_empty(),
                learning: !learn_patterns.is_empty(),
                yes,
                picked: None,
            }
        };
        let meld_sum = meld_with_profiles(
            paths,
            &repo,
            alias,
            roots,
            add_roots,
            flat_skills,
            pin,
            install_hook,
            accept,
            item_kind,
            &mut meld_profiles,
            yes, // spec: DSC-116
            full_history,
        )?;
        // spec: LOBE-7 -- record the consumer's lobe on the new source before
        // anything installs.
        if let Some(l) = lobe.as_deref().filter(|l| !l.trim().is_empty()) {
            set_lobe(paths, &source_ident, l, yes, accept)?;
        }
        // CLI-23: by default, offer to install the melded source's items
        // right away (preview + prompt). `--register-only` stops at registering.
        //
        // CLI-156: in json mode the entire meld+install outcome is folded
        // into ONE JSON object emitted here. Human output is unchanged.
        // spec: PROF-9 PROF-12 PROF-13 -- a profile flag, a source that
        // declares `default`, or a picker answer routes the install offer
        // through profiles; otherwise the CLI-23/CLI-236 paths run
        // unchanged (PROF-7). `meld` asked the picker before the source's
        // install hooks ran (HOOK-142).
        let picked = match meld_profiles {
            MeldProfiles::Consumer { picked, .. } => picked,
            _ => None,
        };
        let use_profiles = !register_only
            && (picked.is_some()
                || meld_uses_profiles(paths, &source_ident, !profile_flags.is_empty())?);
        if use_profiles {
            let selection = picked.unwrap_or(profile_selection);
            let req = ProfileInstall {
                source_name: &source_ident,
                selection: &selection,
                learn_patterns: &learn_patterns,
                only_missing_from_snapshot: false,
                replace: false,
            };
            let outcome = install_source_profiles(paths, &req, flow)?;
            let mut inst = outcome.installed_keys;
            if learn_patterns.is_empty() {
                inst.extend(install_curated_sources(
                    paths,
                    &source_ident,
                    recursive,
                    flow,
                )?);
            } else {
                note_learn_ignores_recursive(recursive);
            }
            if json {
                emit_meld_json_result(meld_sum, inst, outcome.pending, Some(outcome.enabled))?;
            }
        } else if !register_only {
            // Under `--json` every install below is silent and returns its
            // keys, folded into the one meld result (CLI-156).
            let outcome = if learn_patterns.is_empty() {
                let mut outcome = install_source_items(paths, &source_ident, flow)?;
                // DSC-54 installs only the top-level source by default. Walk the
                // curated chain and install each nested source the curator
                // flagged `install = true` (DSC-58), or every nested source with
                // `--recursive` (DSC-55).
                outcome.installed.extend(install_curated_sources(
                    paths,
                    &source_ident,
                    recursive,
                    flow,
                )?);
                outcome
            } else {
                // spec: CLI-236 -- the named subset only: the curated
                // chain (DSC-54/55/58) is not walked, since a pattern
                // selects within the source being melded. Say so when
                // `--recursive` was passed, rather than dropping it
                // silently (the CLI-206 ignored-flag discipline).
                note_learn_ignores_recursive(recursive);
                install_source_items_matching(paths, &source_ident, &learn_patterns, flow)?
            };
            if json {
                emit_meld_json_result(meld_sum, outcome.installed, outcome.pending, None)?;
            }
        } else if json {
            // register-only + json: register only, emit the meld result now.
            emit_meld_json_result(meld_sum, vec![], 0, None)?;
        }
    }
    // spec: HOOK-182 HOOK-183 -- hooks still held back (nothing to install,
    // `--register-only`, or every item already installed) get a hooks-only
    // plan of their own.
    if !meld_aborted() {
        ask_plan(paths, None, Vec::new())?;
    }
    if meld_aborted() {
        return Ok(());
    }
    apply_auto_update_grant(paths, &source_ident, grant, !already_melded, yes, json)?;
    // DSC-56: suggest `mind probe` after melding a curated super-source.
    maybe_probe_hint(paths, &source_ident)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    #[test]
    fn auth_failure_skip_lines_no_message() {
        // spec: DSC-69 -- skip action produces standard line + " (skipping)"
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: None,
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("unable to meld source owner/private-repo"),
            "line: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("(skipping)"),
            "must include (skipping) for skip action: {}",
            lines[0]
        );
    }

    #[test]
    fn auth_failure_skip_lines_with_message() {
        // spec: DSC-69 -- message is printed on the line immediately following
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: Some("Configure credentials: https://example.com/auth".into()),
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("(skipping)"), "first line: {}", lines[0]);
        assert_eq!(lines[1], "Configure credentials: https://example.com/auth");
    }

    #[test]
    fn auth_failure_error_lines_no_skipping_suffix() {
        // spec: DSC-69 -- error action does NOT have " (skipping)" in the message
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Error,
            message: None,
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("unable to meld source"),
            "line: {}",
            lines[0]
        );
        assert!(
            !lines[0].contains("(skipping)"),
            "error action must NOT include (skipping): {}",
            lines[0]
        );
    }

    #[test]
    fn auth_failure_error_lines_with_message_included() {
        // spec: DSC-69 -- message is printed before the process exits non-zero
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Error,
            message: Some("Contact admin for access.".into()),
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 2);
        assert!(
            !lines[0].contains("(skipping)"),
            "error action must NOT include (skipping)"
        );
        assert_eq!(lines[1], "Contact admin for access.");
    }

    #[test]
    fn auth_failure_lines_strips_ansi_escape() {
        // spec: DSC-69 -- ANSI escape bytes in message are stripped before output
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: Some("\x1b[2J hello\x1b[0m".into()),
        };
        let lines = auth_failure_lines("src", &cfg);
        assert_eq!(lines.len(), 2);
        // ANSI bytes are stripped; only printable characters remain.
        assert_eq!(
            lines[1], " hello",
            "expected printable portion only: {:?}",
            lines[1]
        );
        assert!(
            !lines[1].contains('\x1b'),
            "ANSI escape must be stripped: {:?}",
            lines[1]
        );
    }

    /// Build a source named `github.com/acme/<repo>` with the given alias and
    /// persist a registry holding exactly it.
    fn seed_source(paths: &Paths, repo: &str, alias: Option<&str>) -> String {
        let mut src = crate::source::parse_spec(&format!("github:acme/{repo}")).unwrap();
        src.apply_alias(alias.map(|s| s.to_string()));
        let name = src.name.clone();
        let registry = crate::source::Registry { sources: vec![src] };
        registry.save(paths).expect("save registry");
        name
    }

    /// Record an installed item attributed to `source` in the manifest.
    fn seed_installed_item(paths: &Paths, source: &str, kind: ItemKind, name: &str) {
        let mut manifest = Manifest::load(paths).expect("load manifest");
        manifest.insert(InstalledItem {
            kind,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: source.to_string(),
            commit: "deadbeef".to_string(),
            hash: "abc123".to_string(),
            store: format!("store/{name}"),
            links: vec![],
            description: None,
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
            profiles: Vec::new(),
            explicit: false,
            version: None,
            lobe: None,
        });
        manifest.save(paths).expect("save manifest");
    }

    #[test]
    fn set_source_namespace_source_not_found_is_noop() {
        // spec: NS-30 CLI-161 - an unknown source name is a safe no-op (Ok), not
        // an error, and writes nothing.
        let (paths, base) = ns_paths();
        seed_source(&paths, "agents", None);
        let r = set_source_namespace(&paths, "github.com/nope/missing", Some("jk".into()));
        assert!(r.is_ok(), "unknown source must be a no-op Ok: {r:?}");
        // The real source's alias is untouched.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None, "no alias should be written");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_locked_when_items_installed_and_alias_differs() {
        // spec: NS-30 CLI-161 - changing the alias while items are installed is
        // refused with NamespaceLocked naming the installed item keys; nothing
        // is persisted.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        seed_installed_item(&paths, &name, ItemKind::Skill, "review");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        match r {
            Err(MindError::NamespaceLocked { src_name, items }) => {
                assert_eq!(src_name, name);
                assert!(
                    items.contains(&"skill:review".to_string()),
                    "locked error must list the installed item key: {items:?}"
                );
            }
            other => panic!("expected NamespaceLocked, got {other:?}"),
        }
        // The alias must NOT have been persisted despite the requested change.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias, None,
            "a locked change must not write the new alias"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// DSC-95: `NamespaceLocked`'s `items` list (built from each locked item's
    /// `key()`) must reach the caller already sanitized -- `MindError::
    /// NamespaceLocked`'s `#[error(...)]` Display joins `items` verbatim into
    /// the rejection message with no sanitizing step of its own, so a raw ESC
    /// byte in an installed name would otherwise ride straight to a terminal
    /// via `main.rs`'s `eprintln!("error: {err}")`.
    // spec: DSC-95
    #[test]
    fn set_source_namespace_locked_message_sanitizes_installed_item_key() {
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        seed_installed_item(&paths, &name, ItemKind::Skill, "evil\x1b[31mname");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        let msg = match r {
            Err(e @ MindError::NamespaceLocked { .. }) => e.to_string(),
            other => panic!("expected NamespaceLocked, got {other:?}"),
        };
        assert!(!msg.contains('\x1b'), "must strip raw ESC: {msg:?}");
        assert!(msg.contains("evil"), "must still name the item: {msg}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_unchanged_alias_with_items_is_noop() {
        // spec: NS-30 CLI-161 - re-applying the SAME alias is allowed even with
        // items installed: the lock only guards an actual change.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", Some("jk"));
        seed_installed_item(&paths, &name, ItemKind::Skill, "jk:review");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        assert!(
            r.is_ok(),
            "an unchanged alias must be a no-op even with items installed: {r:?}"
        );
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias.as_deref(), Some("jk"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_allowed_and_persisted_when_no_items() {
        // spec: NS-30 CLI-161 - with zero installed items the alias changes and
        // is persisted to sources.json.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, Some("jk".into())).expect("set namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some("jk"),
            "the new alias must be persisted"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_never_sets_identity_alias() {
        // spec: STO-60 -- the fork note's guard checks ONLY `Source.as_alias`
        // (the pre-clone identity alias, STO-58), never the display `alias`.
        // `set_source_namespace` is the same kind of *display-only* prefix
        // mutation as an accepted `[source].prefix` (see its doc comment: "this
        // changes the source's effective display prefix (`alias`), not its
        // identity"). This locks in that `as_alias` stays untouched when the
        // display alias changes, so a source that only ever received a display
        // prefix can never spuriously satisfy the STO-60 fork-note guard
        // (`source.as_alias.as_deref().is_some_and(|a| !a.is_empty())`), no
        // matter how many other instances of the same repo are registered.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, Some("jk".into())).expect("set namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some("jk"),
            "the display alias must change"
        );
        assert_eq!(
            reg.sources[0].as_alias, None,
            "the identity alias must stay None: a display-prefix-only change \
             must never satisfy the STO-60 fork-note guard"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_empty_string_clears_consumer_prefix() {
        // spec: NS-30 CLI-161 - setting an empty alias (the "no prefix" override)
        // persists Some("") which the catalog treats as no consumer prefix.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", Some("jk"));
        set_source_namespace(&paths, &name, Some(String::new())).expect("clear namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some(""),
            "empty string override must persist as Some(\"\")"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_none_equals_empty_is_noop() {
        // spec: NS-30 CLI-161 - None and Some("") are the same "no prefix" state,
        // so requesting None when the current alias is None changes nothing.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, None).expect("noop");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_rejects_reserved_kind_word() {
        // spec: NS-25 NS-30 - validate_prefix rejects a reserved kind word before
        // any registry mutation, returning ReservedPrefix.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        let r = set_source_namespace(&paths, &name, Some("skill".into()));
        assert!(
            matches!(r, Err(MindError::ReservedPrefix { ref prefix }) if prefix == "skill"),
            "a reserved kind word must be rejected: {r:?}"
        );
        // The registry must be untouched.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ----- MKT-10: origin_label mapping -----

    #[test]
    fn origin_label_none_returns_empty() {
        assert_eq!(
            origin_label(None),
            "",
            "no origin -> empty string (no extra token)"
        );
    }

    #[test]
    fn origin_label_claude_plugin_returns_token() {
        let label = origin_label(Some(ManifestOrigin::ClaudePlugin));
        assert_eq!(
            label, " origin:claude-plugin",
            "ClaudePlugin must produce the 'origin:claude-plugin' token with leading space"
        );
    }

    #[test]
    fn origin_label_claude_marketplace_returns_token() {
        let label = origin_label(Some(ManifestOrigin::ClaudeMarketplace));
        assert_eq!(
            label, " origin:claude-marketplace",
            "ClaudeMarketplace must produce the 'origin:claude-marketplace' token with leading space"
        );
    }

    // ----- MKT-7: marketplace_entry_spec helper -----

    #[test]
    fn marketplace_entry_spec_external_passes_spec_through() {
        let entry = plugin_manifest::MarketplaceEntry {
            name: "my-plugin".to_string(),
            source: plugin_manifest::PluginSource::External {
                spec: "owner/my-plugin".to_string(),
                pin: None,
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/some/catalog/clone");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        assert_eq!(
            spec, "owner/my-plugin",
            "External spec must pass through verbatim"
        );
    }

    #[test]
    fn marketplace_entry_spec_in_repo_joins_onto_clone_dir() {
        let entry = plugin_manifest::MarketplaceEntry {
            name: "embedded".to_string(),
            source: plugin_manifest::PluginSource::InRepo {
                path: "plugins/embedded".to_string(),
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/home/user/.mind/sources/local/owner/catalog");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        assert_eq!(
            spec, "/home/user/.mind/sources/local/owner/catalog/plugins/embedded",
            "InRepo spec must be clone_dir joined with the in-repo path"
        );
    }

    #[test]
    fn marketplace_entry_spec_in_repo_dotslash_prefix() {
        // A relative path like "./plugins/p1" (validated safe by load_marketplace_manifest)
        // is joined onto clone_dir and the result is a usable local path.
        let entry = plugin_manifest::MarketplaceEntry {
            name: "p1".to_string(),
            source: plugin_manifest::PluginSource::InRepo {
                path: "./plugins/p1".to_string(),
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/repo");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        // Path::join normalises "./" on the joined string; the key property is
        // that the spec resolves inside clone_dir.
        assert!(
            spec.starts_with("/repo"),
            "InRepo spec with ./ prefix must still be rooted inside clone_dir: {spec}"
        );
    }

    #[test]
    fn collision_prompt_empty_accepts_suggested() {
        // spec: NS-44 — pressing Enter (empty answer) accepts the suggested prefix.
        match parse_collision_answer("", "myrepo") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "myrepo"),
            CollisionAnswer::Abort => panic!("empty must accept suggested, not abort"),
        }
        // Whitespace-only is also empty after trim.
        match parse_collision_answer("   ", "myrepo") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "myrepo"),
            CollisionAnswer::Abort => panic!("whitespace must accept suggested, not abort"),
        }
    }

    #[test]
    fn collision_prompt_dot_aborts() {
        // spec: NS-44 — typing "." aborts the meld (same as non-interactive path).
        assert!(
            matches!(
                parse_collision_answer(".", "myrepo"),
                CollisionAnswer::Abort
            ),
            "dot must abort"
        );
        // Dot with surrounding whitespace is still the abort sentinel.
        assert!(
            matches!(
                parse_collision_answer("  .  ", "myrepo"),
                CollisionAnswer::Abort
            ),
            "dot with whitespace must abort"
        );
    }

    #[test]
    fn collision_prompt_custom_prefix_is_preserved() {
        // spec: NS-44 — typing a custom prefix uses it verbatim (trimmed).
        match parse_collision_answer("mypfx", "suggested") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "mypfx"),
            CollisionAnswer::Abort => panic!("custom prefix must not abort"),
        }
        // Trimming applies.
        match parse_collision_answer("  mypfx  ", "suggested") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "mypfx"),
            CollisionAnswer::Abort => panic!("trimmed custom prefix must not abort"),
        }
    }

    #[test]
    fn format_conflicts_display_strips_ansi_from_name_and_source() {
        // spec: NS-44 — ANSI escape sequences in item name or source name are
        // stripped so a malicious source cannot inject terminal control codes into
        // the interactive collision prompt.
        let conflicts = vec![(
            "skill".to_string(),
            "\x1b[31mreview\x1b[0m".to_string(), // ANSI-colored name
            "\x1b[32mevil/source\x1b[0m".to_string(), // ANSI-colored source
        )];
        let out = format_conflicts_display(&conflicts);
        assert!(
            out.contains("skill:review"),
            "bare name must appear without ANSI: {out}"
        );
        assert!(
            out.contains("evil/source"),
            "bare source must appear without ANSI: {out}"
        );
        assert!(
            !out.contains('\x1b'),
            "no ANSI escape bytes must appear in the output: {out}"
        );
    }

    // ----- STO-69: clone-path confinement -----
}
