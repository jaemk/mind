//! The optional `mind.toml` a source repo may place at its root to declare its
//! inventory explicitly, instead of relying on convention scanning.
//!
//! Everything is optional. A repo with no `mind.toml` is scanned by convention.
//! A repo with only `[source]` metadata still gets convention scanning. A repo
//! that declares `[[items]]` or `[discover]` opts out of convention scanning and
//! becomes authoritative for its own inventory.

use std::path::Path;

use serde::Deserialize;

use crate::error::{ItemKind, MindError, Result};
use crate::git::validate_ref_value;
use crate::source::Pin;
use crate::version::{Template, Version};

/// The lifecycle event a hook is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Install,
    /// Runs at `upgrade` in place of the install hooks (HOOK-120..122), for a
    /// setup step that must differ when the thing it belongs to is updated
    /// rather than first installed.
    Update,
    Uninstall,
}

impl HookEvent {
    /// The event's spelling in `mind.toml`, in a disclosure, and in a note.
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::Install => "install",
            HookEvent::Update => "update",
            HookEvent::Uninstall => "uninstall",
        }
    }
}

/// One raw `[[hooks]]` entry as deserialized from `mind.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub run: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub optional: bool,
    /// Raw event string; validated by `resolved_hooks` ("install" | "update" |
    /// "uninstall", default "install"). Kept as a string so an unknown value is
    /// a clear mind.toml schema error rather than an opaque serde failure.
    #[serde(default)]
    pub event: Option<String>,
}

/// A validated, normalized hook ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHook {
    pub run: String,
    pub name: Option<String>,
    pub optional: bool,
    pub event: HookEvent,
}

impl ResolvedHook {
    /// The label shown in disclosures: the name if non-empty, else the command.
    /// An empty or whitespace-only name is treated as absent (HOOK-51).
    pub fn label(&self) -> &str {
        match self.name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.run,
        }
    }
}

/// The parsed `mind.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MindToml {
    #[serde(default)]
    pub source: SourceMeta,
    /// Explicit inventory; authoritative when non-empty.
    #[serde(default)]
    pub items: Vec<ItemDecl>,
    /// Glob-based discovery; authoritative when present.
    pub discover: Option<Discover>,
    /// Declared hooks in declaration order (HOOK-51).
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Install profiles (PROF-1): profile name to members. Metadata like
    /// `[source]`: it never makes the file authoritative. Validated at load
    /// (PROF-2..4) by [`crate::profile::validate_table`].
    pub profiles: Option<crate::profile::ProfileTable>,
}

/// Repo-level metadata.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMeta {
    pub description: Option<String>,
    /// An install hook (HOOK-1): a shell command the maintainer declares to build
    /// or install the tooling this source's items rely on. Run on `meld` after
    /// checkout, gated by a safety prompt (see spec/install-hooks.md). `None` when
    /// the source declares no hook.
    pub install: Option<String>,
    /// Namespace prefix applied to every item from this source (see
    /// [`crate::namespace`]). A consumer `meld --namespace` overrides it.
    /// `namespace` is the canonical key; the legacy `prefix` spelling still
    /// parses as a deprecated alias (DSC-82).
    #[serde(rename = "namespace", alias = "prefix")]
    pub prefix: Option<String>,
    /// The minimum `mind` version this repo expects. Enforced at scan/meld time:
    /// a source requiring a newer `mind` than the one running is rejected (see
    /// [`version_at_least`] and `catalog::scan`).
    #[serde(rename = "min-mind-version")]
    pub min_mind_version: Option<String>,
    /// Pin directive: track a named branch (DSC-41).
    #[serde(rename = "follow-branch")]
    pub follow_branch: Option<String>,
    /// Pin directive: fix to a tag (DSC-41).
    #[serde(rename = "pin-tag")]
    pub pin_tag: Option<String>,
    /// Pin directive: fix to a specific commit (DSC-41).
    #[serde(rename = "pin-ref")]
    pub pin_ref: Option<String>,
    /// Pin directive: follow release tags within a version range (VER-51).
    #[serde(rename = "pin-version")]
    pub pin_version: Option<String>,
    /// Convention scan roots (DSC-50). When set, convention discovery scans
    /// under each listed repo-root-relative directory instead of the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    /// Ignore patterns applied to every item of this source (IGN-1), matched
    /// against paths relative to each ITEM's own path, not the repo root. An
    /// item's own `[[items]].ignore` replaces this list for that item.
    #[serde(default)]
    pub ignore: Option<Vec<String>>,
    /// Flat skill layout (DSC-74). When true, convention discovery finds skills
    /// as bare-name directories with a direct `SKILL.md` under each scan root,
    /// with no `skills/` container. Default false (the DSC-10 container behavior).
    #[serde(rename = "flat-skills", default)]
    pub flat_skills: bool,
    /// Release tag template (VER-1): `true` for the default template, or a
    /// string with exactly one `{version}`. Metadata only: it does not make
    /// the file authoritative.
    #[serde(rename = "release-tags")]
    pub release_tags: Option<ReleaseTags>,
    /// Versions that version-pin resolution never selects (VER-46).
    pub yanked: Option<Vec<String>>,
    /// The highest auto-update level the maintainer wants (AUP-5): `true`,
    /// `"compatible"`, or `"breaking"`.
    #[serde(rename = "auto-update")]
    pub auto_update: Option<RawAutoUpdate>,
}

/// The raw `[source].auto-update` value, before
/// [`SourceMeta::auto_update_level`] validates it.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RawAutoUpdate {
    Flag(bool),
    Level(String),
}

/// The raw `[source].release-tags` value, before [`SourceMeta::release_template`]
/// validates it.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ReleaseTags {
    Flag(bool),
    Template(String),
}

/// `[source].pin-version` as the range it means (VER-53): a source recommends
/// a range, so an exact version reads as its caret (`=1.2.3` is `^1.2.3`).
/// Holding one release is the consumer's call (`--pin version==X.Y.Z`).
// spec: VER-53
fn source_range(range: crate::version::Range) -> crate::version::Range {
    match range {
        crate::version::Range::Exact(v) => crate::version::Range::Caret {
            major: v.major,
            minor: Some(v.minor),
            patch: Some(v.patch),
            pre: v.pre,
        },
        caret => caret,
    }
}

impl SourceMeta {
    /// The declared release tag template, or `None` when `release-tags` is
    /// absent. A malformed template, or `release-tags = false`, is a
    /// `MindToml` error (VER-2).
    // spec: VER-1 VER-2
    pub fn release_template(&self, toml_path: &Path) -> Result<Option<Template>> {
        let bad = |msg: String| MindError::MindToml {
            path: toml_path.to_path_buf(),
            msg,
        };
        match &self.release_tags {
            None => Ok(None),
            Some(ReleaseTags::Flag(true)) => Ok(Some(Template::Default)),
            Some(ReleaseTags::Flag(false)) => Err(bad(
                "release-tags must be true or a template string such as \"v{version}\"".into(),
            )),
            Some(ReleaseTags::Template(s)) => Template::parse(s).map(Some).map_err(bad),
        }
    }

    /// The `[source].yanked` versions. An entry that is not a semver version
    /// is a `MindToml` error (VER-46).
    // spec: VER-46
    pub fn yanked_versions(&self, toml_path: &Path) -> Result<Vec<Version>> {
        self.yanked
            .iter()
            .flatten()
            .map(|s| {
                Version::parse(s).ok_or_else(|| MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!("yanked entry {s:?} is not a semver version (MAJOR.MINOR.PATCH)"),
                })
            })
            .collect()
    }

    /// The declared auto-update level (AUP-5): `true` is `compatible`; `false`,
    /// `"off"`, or any other value is a `MindToml` error.
    // spec: AUP-5
    pub fn auto_update_level(&self, toml_path: &Path) -> Result<Option<crate::auto_update::Level>> {
        use crate::auto_update::Level;
        let bad = |shown: String| MindError::MindToml {
            path: toml_path.to_path_buf(),
            msg: format!("auto-update {shown} must be true, \"compatible\", or \"breaking\""),
        };
        match &self.auto_update {
            None => Ok(None),
            Some(RawAutoUpdate::Flag(true)) => Ok(Some(Level::Compatible)),
            Some(RawAutoUpdate::Flag(false)) => Err(bad("false".into())),
            Some(RawAutoUpdate::Level(s)) => match Level::parse(s) {
                Some(Level::Off) | None => {
                    Err(bad(format!("{:?}", crate::sanitize::strip_ansi(s))))
                }
                Some(level) => Ok(Some(level)),
            },
        }
    }

    /// The range the author recommends (VER-140): `pin-version`, when it
    /// parses, with an exact version read as its caret (VER-53). Read
    /// leniently, so a typo there never fails a `sync`.
    pub fn recommended_range(&self) -> Option<crate::version::Range> {
        self.pin_version
            .as_deref()
            .and_then(crate::version::Range::parse)
            .map(source_range)
    }

    /// Whether `pin-version` is an exact version, which reads as its caret
    /// (VER-53); `review` says so.
    pub fn pin_version_is_exact(&self) -> bool {
        self.pin_version
            .as_deref()
            .and_then(crate::version::Range::parse)
            .is_some_and(|r| matches!(r, crate::version::Range::Exact(_)))
    }

    /// Return the single pin declared by this `[source]` section, or `None`
    /// if none is declared. More than one declared pin, or a `pin-version`
    /// range that does not parse, is a `MindToml` error (DSC-41, VER-51).
    // spec: VER-51
    pub fn pin_directive(&self, toml_path: &Path) -> Result<Option<Pin>> {
        // Collect which directives are set so we can detect conflicts and give
        // a useful error message naming both conflicting keys.
        let mut set: Vec<(&str, Pin)> = Vec::new();
        if let Some(b) = &self.follow_branch {
            validate_ref_value(b)?;
            set.push(("follow-branch", Pin::FollowBranch(b.clone())));
        }
        if let Some(t) = &self.pin_tag {
            validate_ref_value(t)?;
            set.push(("pin-tag", Pin::Tag(t.clone())));
        }
        if let Some(r) = &self.pin_ref {
            validate_ref_value(r)?;
            set.push(("pin-ref", Pin::Ref(r.clone())));
        }
        if let Some(v) = &self.pin_version {
            let range = crate::version::Range::parse(v).ok_or_else(|| MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!("pin-version {v:?} is not a version range (such as \"^1.2\")"),
            })?;
            let range = source_range(range);
            set.push((
                "pin-version",
                Pin::Version(crate::source::VersionPin::unresolved(&range)),
            ));
        }
        match set.len() {
            0 => Ok(None),
            1 => Ok(Some(set.remove(0).1)),
            _ => {
                let names: Vec<&str> = set.iter().map(|(k, _)| *k).collect();
                Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "conflicting pin directives: {}; declare at most one of follow-branch, pin-tag, pin-ref, pin-version",
                        names.join(", ")
                    ),
                })
            }
        }
    }
}

/// One explicitly declared item.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDecl {
    /// `skill`, `agent`, `rule`, `command` (CMD-4), `workflow` (WF-1), or
    /// `tool`.
    pub kind: String,
    pub name: String,
    /// Path to the item, relative to the repo root (a dir for skills/tools).
    ///
    /// Shape is checked against the kind where the path meets the filesystem,
    /// in `catalog::from_decl`, not here: this reader is a pure parse of the
    /// TOML and has no clone to look in. A `workflow` entry whose path names a
    /// directory is refused there (DSC-109), since a workflow is exactly one
    /// `.js` file.
    pub path: String,
    /// Optional override for where to link this item within each agent home
    /// (lobe). The value is a path relative to each lobe root
    /// (e.g. `rules/style.md`), and is applied uniformly to every lobe that
    /// admits this item's kind: the symlink lands at `<lobe-home>/<link>`.
    /// With cross-harness lobes (Claude, Gemini, Codex, etc.) the same
    /// relative path is resolved against every admitted lobe. Absent means
    /// use the default location for the kind in each lobe.
    pub link: Option<String>,
    /// Optional description override (else taken from frontmatter).
    pub description: Option<String>,
    /// A tool's entrypoint, relative to its dir (what `{{tools:name}}` resolves
    /// to). Tool items only; else a `mind.toml` schema error.
    pub bin: Option<String>,
    /// A tool's per-item build command, run in staging at install. Tool items
    /// only; else a `mind.toml` schema error.
    pub build: Option<String>,
    /// An item install hook (HOOK-80): a shell command run on the host as the
    /// final step of installing this item (after the store swap and links), for
    /// side effects (set up a venv, register a helper). Valid on any kind. `None`
    /// means no install hook.
    pub install: Option<String>,
    /// An item update hook (HOOK-120, HOOK-125): a shell command run in place of
    /// the item's install hooks when the item is re-installed over an existing
    /// install. Valid on any kind. `None` means no update hook.
    pub update: Option<String>,
    /// An item uninstall hook (HOOK-80): a shell command run when this item is
    /// removed (at `forget`/`unmeld`/rename-on-upgrade), before its store copy
    /// and links are removed. Valid on any kind. `None` means no uninstall hook.
    pub uninstall: Option<String>,
    /// Item lifecycle hooks declared as an array (HOOK-86), the per-item analog
    /// of the source `[[hooks]]` array (HOOK-50): the field is `[[items.hooks]]`
    /// in `mind.toml`. Same fields/semantics as a source hook. The scalar
    /// `install`/`update`/`uninstall` above are folded in ahead of these (see
    /// [`ItemDecl::resolved_item_hooks`]).
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Ignore patterns for this item (IGN-1), matched against paths relative to
    /// the item's own path. When present this REPLACES `[source].ignore` for
    /// this item rather than adding to it, matching how `[[items]]` overrides
    /// `[source]` elsewhere. The built-in VCS set (IGN-2) applies either way.
    #[serde(default)]
    pub ignore: Option<Vec<String>>,
}

impl ItemDecl {
    /// Resolve this item's lifecycle hooks (HOOK-86) in execution order.
    ///
    /// Mirrors [`MindToml::resolved_hooks`]: each scalar folds in as the first
    /// required hook of its event -- `install` (HOOK-80 shorthand), then
    /// `update` (HOOK-120), then `uninstall` -- all ahead of the
    /// `[[items.hooks]]` entries, which are then validated and appended in
    /// declaration order. An empty/whitespace `run` is dropped (HOOK-3) and an
    /// unknown `event` is a `mind.toml` schema error.
    pub fn resolved_item_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        let mut out: Vec<ResolvedHook> = Vec::new();

        // HOOK-86: fold the scalar install/update/uninstall shorthand in first,
        // each as a required hook of its event.
        for (scalar, event) in [
            (&self.install, HookEvent::Install),
            (&self.update, HookEvent::Update),
            (&self.uninstall, HookEvent::Uninstall),
        ] {
            if let Some(cmd) = scalar {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    out.push(ResolvedHook {
                        run: trimmed.to_owned(),
                        name: None,
                        optional: false,
                        event,
                    });
                }
            }
        }

        // HOOK-86: [[items.hooks]] entries in declaration order, validated the
        // same way as a source's [[hooks]].
        out.extend(self.resolved_item_array_hooks(toml_path)?);

        Ok(out)
    }

    /// Whether this declaration says anything at all about the item's hooks: a
    /// scalar `install`/`update`/`uninstall`, or an `[[items.hooks]]` entry.
    ///
    /// A declaration that does is authoritative for the item's hooks and
    /// suppresses the other two declaration sites (HOOK-132); one that does not
    /// leaves the item free to declare its own (an item-directory `mind.toml`,
    /// else frontmatter). Blank values do not count: they are absent (HOOK-3).
    // spec: HOOK-132
    pub fn declares_hooks(&self) -> bool {
        let scalar = [&self.install, &self.update, &self.uninstall]
            .into_iter()
            .flatten()
            .any(|c| !c.trim().is_empty());
        scalar || self.hooks.iter().any(|h| !h.run.trim().is_empty())
    }

    /// Resolve the `[[items.hooks]]` array entries (HOOK-86), validated in
    /// declaration order. Returns only the array-declared hooks; the scalar
    /// `install`/`uninstall` fold-in is handled by the caller
    /// (`resolved_item_hooks`), which prepends those scalars ahead of this
    /// result.
    fn resolved_item_array_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        resolve_hook_array(&self.hooks, toml_path, &format!("item '{}'", self.name))
    }
}

/// The optional per-item `mind.toml` a directory-backed item (a skill or a
/// tool) may ship in its own directory (HOOK-131).
///
/// A scoped-down manifest: `[[hooks]]` is the only table it accepts, with
/// exactly the fields and semantics of a source's `[[hooks]]` (HOOK-51). Any
/// other key is a schema error (`deny_unknown_fields`), so a source manifest
/// copied into an item directory fails loudly instead of being half-read.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemToml {
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Accepted by the parser only so [`ItemToml::load`] can refuse it with a
    /// message naming the root-only rule (PROF-8), instead of the generic
    /// unknown-key error. Never read otherwise.
    #[serde(default)]
    profiles: Option<toml::Value>,
}

impl ItemToml {
    /// Read `<item_dir>/mind.toml`, or `Ok(None)` when the item ships none.
    ///
    /// Size-capped like every other metadata read (DSC-91). A parse failure or
    /// an unknown key is an error naming the file and the one-table rule: an
    /// item manifest exists only to declare code that will run on the host, so
    /// a malformed one must not degrade to "this item declares no hooks". The
    /// caller (`catalog::item_declared_hooks`) scopes that failure to the one
    /// item, dropping it from the catalog (DSC-98).
    ///
    /// spec: HOOK-131
    /// spec: DSC-95 -- the error carries the file's own path and the `toml`
    /// crate's annotated snippet, which quotes the offending input line
    /// verbatim. Both are source-controlled (a directory name, the file's
    /// bytes) and a raw ESC byte in a TOML file is itself a parse error, so the
    /// snippet is the path a hostile source would use; sanitize both here,
    /// where the message is composed, rather than at each print site.
    pub fn load(item_dir: &Path) -> Result<Option<ItemToml>> {
        let file = item_dir.join("mind.toml");
        let text = match crate::error::read_capped_metadata(&file) {
            Ok(text) => text,
            Err(MindError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        let parsed: ItemToml = toml::from_str(&text).map_err(|e| MindError::MindToml {
            path: sanitized_path(&file),
            msg: format!(
                "not readable as an item manifest, whose only table is `[[hooks]]` (any other \
                 key is rejected): {}",
                crate::sanitize::strip_ansi(&e.to_string())
            ),
        })?;
        // spec: PROF-8 -- profiles select among a source's items, so only the
        // root mind.toml may declare them.
        if parsed.profiles.is_some() {
            return Err(MindError::MindToml {
                path: sanitized_path(&file),
                msg: "[profiles] is not allowed in an item manifest; profiles are read from the \
                      source's root mind.toml only"
                    .to_string(),
            });
        }
        Ok(Some(parsed))
    }

    /// This manifest's hooks, validated and in declaration order (HOOK-131).
    /// `path` is the item manifest's own path, so an error localizes to it.
    pub fn resolved_hooks(&self, path: &Path) -> Result<Vec<ResolvedHook>> {
        resolve_hook_array(&self.hooks, path, "item manifest")
    }
}

/// Validate and normalize a slice of raw `Hook` entries into `ResolvedHook`s
/// in declaration order. Used by `MindToml::resolved_hooks`,
/// `ItemDecl::resolved_item_array_hooks`, and `NestedSource::resolved_hooks`.
///
/// `location` names the `mind.toml` section the hooks belong to (e.g. `item
/// 'style'`, `[source]`, `nested source 'owner/repo'`) so a hook error localizes
/// which declaration is at fault instead of just naming the file.
///
/// Rules: default event is Install; "update" maps to Update (HOOK-120) and
/// "uninstall" to Uninstall; any other event string is a `MindToml` error.
/// Empty/whitespace `run` entries are silently dropped (HOOK-3). `run` is
/// trimmed before storing.
fn resolve_hook_array(
    hooks: &[Hook],
    toml_path: &std::path::Path,
    location: &str,
) -> Result<Vec<ResolvedHook>> {
    let mut out: Vec<ResolvedHook> = Vec::new();
    for hook in hooks {
        let run = hook.run.trim();
        if run.is_empty() {
            continue; // HOOK-3 generalized: skip blank run
        }
        let event = match hook.event.as_deref() {
            None | Some("install") => HookEvent::Install,
            // spec: HOOK-120
            Some("update") => HookEvent::Update,
            Some("uninstall") => HookEvent::Uninstall,
            Some(e) => {
                // spec: DSC-95 -- the event string is source-controlled and is
                // echoed back here, on a message that reaches stderr (as a
                // scan error, or as the DSC-98 item-drop warning).
                let e = crate::sanitize::strip_ansi(e);
                return Err(MindError::MindToml {
                    path: sanitized_path(toml_path),
                    msg: format!(
                        "{location}: unknown hook event '{e}'; expected 'install', 'update', or 'uninstall'"
                    ),
                });
            }
        };
        out.push(ResolvedHook {
            run: run.to_owned(),
            name: hook.name.clone(),
            optional: hook.optional,
            event,
        });
    }
    Ok(out)
}

/// Glob-based discovery: per-kind include/exclude, plus nested sources.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discover {
    #[serde(default)]
    pub skills: KindGlobs,
    #[serde(default)]
    pub agents: KindGlobs,
    #[serde(default)]
    pub rules: KindGlobs,
    /// Command globs match the command FILE (`commands/<name>.md`), as the agent
    /// and rule globs do (commands.md CMD-4).
    #[serde(default)]
    pub commands: KindGlobs,
    /// Workflow globs match the workflow FILE (`workflows/<name>.js`), as the
    /// agent, rule, and command globs do (workflows.md WF-6).
    #[serde(default)]
    pub workflows: KindGlobs,
    /// Tool globs match the tool DIRECTORY (e.g. `packages/*/tool`), not an
    /// anchor file: the matched directory is the tool.
    #[serde(default)]
    pub tools: KindGlobs,
    /// Resource globs match the resource entry itself, a file or a directory
    /// (resources.md RES-4).
    #[serde(default)]
    pub resources: KindGlobs,
    /// Other sources this repo curates. Melding this repo recursively melds each
    /// (see commands::meld), so a `mind.toml` can act as a registry / super-source.
    #[serde(default)]
    pub sources: Vec<NestedSource>,
}

/// Include/exclude glob patterns for one kind, relative to the repo root. An
/// item matched by `include` is kept unless it is also matched by `exclude`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindGlobs {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// The action taken when a nested source's clone fails with an auth error (DSC-68).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthFailureAction {
    Skip,
    Error,
}

/// Auth-failure policy for a nested source (DSC-68). When the nested source's
/// clone fails with a credential-denial error, `action` (`"error"` or `"skip"`)
/// governs whether `meld` exits non-zero or warns and continues, and `message`
/// is an optional line shown to the user alongside the standard auth-failure line.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnAuthFailure {
    pub action: AuthFailureAction,
    #[serde(default)]
    pub message: Option<String>,
}

/// A source referenced by a curated super-source.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NestedSource {
    /// A repo spec, parsed exactly like a `meld` argument.
    pub source: String,
    /// Canonical namespace key (DSC-78): namespace to impose on the nested source (like `meld --as`).
    #[serde(rename = "namespace", default)]
    pub namespace: Option<String>,
    /// Legacy alias key kept for backwards compatibility (DSC-78). Prefer `namespace`.
    #[serde(rename = "as", default)]
    pub alias: Option<String>,
    /// The kind of the item an item-link `source` names (DSC-100, LNK-21):
    /// `agent`, `rule`, or `command`. Read as a string here and parsed by
    /// [`NestedSource::item_kind`] so an unknown value is a `MindToml` error
    /// naming the entry, not an opaque deserialize failure.
    #[serde(default)]
    pub kind: Option<String>,
    /// When true, melding the super-source offers this nested source's items for
    /// install (the curator recommends installing it), instead of leaving them
    /// only registered and available. Default false (DSC-58).
    #[serde(default)]
    pub install: bool,
    /// A subset of the nested source's items to offer for install (DSC-62).
    /// Bare `kind:name` refs in source truth; a prefix in effect for the entry
    /// is applied at install time. `None` means this field is absent (fall back
    /// to the `install` boolean). `Some([])` is equivalent to `install = false`
    /// (offer nothing). Setting this alongside `install = true` is an error
    /// (DSC-64).
    #[serde(rename = "install-items", default)]
    pub install_items: Option<Vec<String>>,
    /// Profiles of the nested source to enable at install (PROF-26), as if
    /// given as `--profile`. Composes with `install-items` (the union is
    /// installed); setting it alongside `install = true` is an error.
    #[serde(default)]
    pub profiles: Option<Vec<String>>,
    /// `false` acts as `--no-default-profile` for the nested source (PROF-26).
    /// `None` and `Some(true)` keep `default` enabled.
    #[serde(rename = "default-profile", default)]
    pub default_profile: Option<bool>,
    /// Curator-supplied pin: track a named branch (DSC-59, DSC-41 one-of).
    #[serde(rename = "follow-branch", default)]
    pub follow_branch: Option<String>,
    /// Curator-supplied pin: fix to a named tag (DSC-59, DSC-41 one-of).
    #[serde(rename = "pin-tag", default)]
    pub pin_tag: Option<String>,
    /// Curator-supplied pin: fix to a specific commit sha (DSC-59, DSC-41
    /// one-of). Also emitted by `mind dump` (DUMP-1, DUMP-4, DSC-65).
    #[serde(rename = "pin-ref", default)]
    pub pin_ref: Option<String>,
    /// Curator-supplied pin: follow release versions in a range (VER-110,
    /// DSC-41 one-of).
    #[serde(rename = "pin-version", default)]
    pub pin_version: Option<String>,
    /// Curator-supplied convention scan roots (DSC-59 / DSC-50 shape): when
    /// set, convention discovery scans under each listed directory instead of
    /// the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    /// Curator-supplied `--add-root` convention roots (STO-55, DUMP-11): extra
    /// roots that compose with the nested source's discovery layer. Unlike
    /// `roots`, these are NOT gated by DSC-60 (add-root always composes, DSC-84);
    /// they are threaded to the nested meld as the consumer `--add-root` override.
    /// Emitted by `mind dump` (DUMP-11).
    #[serde(rename = "add-roots", default)]
    pub add_roots: Option<Vec<String>>,
    /// Curator-supplied flat skill layout (DSC-77): when true, the nested source
    /// uses the flat skill layout (DSC-74). Like `roots`, gated by DSC-60: applied
    /// only when the nested source ships no `mind.toml` of its own.
    #[serde(rename = "flat-skills", default)]
    pub flat_skills: bool,
    /// Curator-supplied lifecycle hooks (DSC-59). Addressed in mind.toml as
    /// `[[discover.sources.hooks]]` entries; validated the same way as a
    /// source-level `[[hooks]]` array.
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Auth failure policy for this nested source (DSC-68). When set and the
    /// source's clone fails with credential-denial errors, `action` governs
    /// whether to skip or error, and `message` is shown to the user.
    #[serde(rename = "on-auth-failure", default)]
    pub on_auth_failure: Option<OnAuthFailure>,
}

impl NestedSource {
    /// Return the effective namespace alias for this nested source (DSC-78).
    ///
    /// `namespace` is the canonical key; `as` is the legacy alias. When both
    /// are set, `namespace` wins. Returns `None` when neither is set.
    pub fn effective_alias(&self) -> Option<String> {
        self.namespace.clone().or_else(|| self.alias.clone())
    }

    /// The parsed `kind =` of this entry (DSC-100), or `None` when absent.
    ///
    /// An unknown kind word is a `MindToml` error naming the entry and the
    /// legal set. Whether the kind FITS the entry's link shape (LNK-21) is
    /// decided later, against the clone.
    pub fn item_kind(&self, toml_path: &Path) -> Result<Option<ItemKind>> {
        let Some(raw) = self
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        else {
            return Ok(None);
        };
        ItemKind::parse(raw)
            .map(Some)
            .ok_or_else(|| MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!(
                    "nested source '{}': unknown kind '{}'; expected agent, rule, or command",
                    crate::sanitize::strip_ansi(&self.source),
                    crate::sanitize::strip_ansi(raw)
                ),
            })
    }

    /// Validate this entry for mutual-exclusion constraints (DSC-64).
    ///
    /// `install = true` together with a non-empty `install_items` list is a
    /// `MindToml` error: offering all and offering a named subset are mutually
    /// exclusive.
    pub fn validate(&self, toml_path: &Path) -> Result<()> {
        if self.install
            && self
                .install_items
                .as_ref()
                .is_some_and(|items| !items.is_empty())
        {
            return Err(MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!(
                    "nested source '{}': install = true and install-items are mutually exclusive; \
                     use install-items alone to offer a subset, or install = true to offer all",
                    self.source
                ),
            });
        }
        // spec: PROF-26 -- `profiles` is a subset directive like
        // `install-items`, so it conflicts with `install = true` the same way
        // (DSC-64), and each name must be well formed (PROF-2).
        if let Some(profiles) = &self.profiles {
            if self.install && !profiles.is_empty() {
                return Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "nested source '{}': install = true and profiles are mutually exclusive; \
                         use profiles alone to offer those profiles, or install = true to offer \
                         the source's default",
                        crate::sanitize::strip_ansi(&self.source)
                    ),
                });
            }
            for name in profiles {
                if let Some(reason) = crate::profile::name_problem(name) {
                    return Err(MindError::MindToml {
                        path: toml_path.to_path_buf(),
                        msg: format!(
                            "nested source '{}': profile name '{}' {reason}",
                            crate::sanitize::strip_ansi(&self.source),
                            crate::sanitize::strip_ansi(name)
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Return the curator-supplied pin directive, or `None` when not set.
    ///
    /// Accepts any of `follow-branch`, `pin-tag`, `pin-ref`, or `pin-version`
    /// (DSC-59, DSC-41 one-of rule, VER-110). Declaring more than one, or a
    /// `pin-version` that does not parse, is a `MindToml` error.
    pub fn pin_directive(&self, toml_path: &Path) -> Result<Option<Pin>> {
        // spec: DSC-59 DSC-65 DSC-66
        let mut set: Vec<(&str, Pin)> = Vec::new();
        if let Some(b) = &self.follow_branch {
            validate_ref_value(b)?;
            set.push(("follow-branch", Pin::FollowBranch(b.clone())));
        }
        if let Some(t) = &self.pin_tag {
            validate_ref_value(t)?;
            set.push(("pin-tag", Pin::Tag(t.clone())));
        }
        if let Some(r) = &self.pin_ref {
            validate_ref_value(r)?;
            set.push(("pin-ref", Pin::Ref(r.clone())));
        }
        if let Some(v) = &self.pin_version {
            let range = crate::version::Range::parse(v).ok_or_else(|| MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!(
                    "nested source '{}': pin-version {v:?} is not a version range (such as \"^1.2\" or \"=1.2.3\")",
                    self.source
                ),
            })?;
            set.push((
                "pin-version",
                Pin::Version(crate::source::VersionPin::unresolved(&range)),
            ));
        }
        match set.len() {
            0 => Ok(None),
            1 => Ok(Some(set.remove(0).1)),
            _ => {
                let names: Vec<&str> = set.iter().map(|(k, _)| *k).collect();
                Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "nested source '{}': conflicting pin directives: {}; declare at most one of follow-branch, pin-tag, pin-ref, pin-version",
                        self.source,
                        names.join(", ")
                    ),
                })
            }
        }
    }

    /// Resolve this nested source's curator-supplied lifecycle hooks in
    /// declaration order.
    ///
    /// Validation rules mirror the source-level `[[hooks]]` (see
    /// `MindToml::resolved_hooks`): the default event is Install, explicit
    /// "uninstall" maps to Uninstall, any other event string is a `MindToml`
    /// error naming the bad value and the legal set. Empty/whitespace `run`
    /// entries are silently dropped (HOOK-3). There is no legacy
    /// `[source].install` fold-in here; that is a MindToml-only concern.
    pub fn resolved_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        resolve_hook_array(
            &self.hooks,
            toml_path,
            &format!("nested source '{}'", self.source),
        )
    }
}

impl Discover {
    /// Every per-kind glob list this section can carry, in kind order.
    ///
    /// The single enumeration of the kinds: `has_item_globs` and
    /// `validate_discover_patterns` both read it, so a kind added to
    /// [`Discover`] cannot be silently honored by one and skipped by the other
    /// (which is exactly how `commands` first landed: discovered by the scan,
    /// but neither authoritative nor validated).
    fn kind_globs(&self) -> [&KindGlobs; 7] {
        [
            &self.skills,
            &self.agents,
            &self.rules,
            &self.commands,
            &self.workflows,
            &self.tools,
            &self.resources,
        ]
    }

    /// Whether this section declares item globs (as opposed to only nested
    /// sources). Item globs turn off convention discovery; a bare `sources` list
    /// does not.
    pub fn has_item_globs(&self) -> bool {
        self.kind_globs().iter().any(|g| !g.include.is_empty())
    }
}

/// Validate every glob pattern in a `[discover]` section (DSC-81): patterns
/// must be relative (no leading `/` or `~`) and must not contain `..` components.
/// Called at parse time in [`MindToml::load`].
fn validate_discover_patterns(discover: &Discover, toml_path: &Path) -> Result<()> {
    let all_globs: Vec<&str> = discover
        .kind_globs()
        .into_iter()
        .flat_map(|g| g.include.iter().chain(g.exclude.iter()))
        .map(String::as_str)
        .collect();

    for pattern in all_globs {
        let pp = std::path::Path::new(pattern);
        // DSC-95: the pattern is source-controlled and this message reaches the
        // terminal, so echo it stripped. The rejection tests the RAW pattern;
        // only the rendering is sanitized.
        let shown = crate::sanitize::strip_ansi(pattern);
        if pp.is_absolute() || pattern.starts_with('~') {
            return Err(MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!(
                    "discover glob '{shown}' is absolute; globs must be relative to the repo root"
                ),
            });
        }
        use std::path::Component;
        if pp.components().any(|c| c == Component::ParentDir) {
            return Err(MindError::MindToml {
                path: toml_path.to_path_buf(),
                msg: format!(
                    "discover glob '{shown}' contains '..'; globs must not escape the repo root"
                ),
            });
        }
    }
    Ok(())
}

/// Returns `true` if `s` is a dotted numeric version string acceptable as a
/// version pin or `min-mind-version` value (e.g. "1", "0.7", "2.3.1").
/// Each dot-separated component must be a non-empty run of ASCII digits.
pub fn is_plausible_version(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
}

/// Returns `true` if `s` is safe to interpolate as a single URL path segment
/// naming a resolved `evolve` release tag (STO-76): a dotted numeric version
/// (as accepted by [`is_plausible_version`]) optionally followed by a
/// semver-shaped prerelease suffix (`-...`) and/or build-metadata suffix
/// (`+...`), e.g. `"1.2.3"`, `"1.2.3-rc1"`, `"1.2.3-rc.2"`, `"1.2.3+build.5"`.
///
/// This is a DIFFERENT question from [`is_plausible_version`], which answers
/// "is this a valid `min-mind-version` / policy version pin" and correctly
/// excludes a prerelease suffix there (meaningless for those fields). Reusing
/// that validator for a resolved `evolve` target refuses a legitimate
/// `evolve --to 1.2.3-rc1` outright -- testing a prerelease before promoting
/// it is a legitimate thing to want, and since GitHub's `releases/latest`
/// excludes prereleases, an explicit `--to` is the only way to reach one. This
/// validator accepts that shape while still rejecting anything that could
/// escape or split a URL path segment: `/`, `\`, whitespace, control
/// characters, and -- critically -- a run of two or more consecutive `.`
/// characters ANYWHERE in the string, including inside the suffix (e.g.
/// `1.0.0-../..`), which would otherwise smuggle a `..` traversal segment into
/// the release-asset or `SHA256SUMS` URL once curl normalizes it.
///
/// The `-`/`+` suffixes are validated the same way as the base version: split
/// on `.`, every component must be non-empty. A `..` produces an empty
/// component between the two dots, so it is rejected there exactly as it is
/// in the base version, and any character outside `[0-9A-Za-z-]` (in
/// particular `/` and `\`) fails the per-component check regardless of `.`
/// placement.
///
/// Grammar (informally, an IDENTIFIER LIST, not a bare character class): the
/// base is `\d+(\.\d+)*`; each optional suffix is `ident(\.ident)*` with
/// `ident = [0-9A-Za-z-]+` (one or more of that charset, so EVERY
/// dot-separated identifier must itself be non-empty). This is deliberately
/// NOT the same shape as `(-[0-9A-Za-z.-]+)?` (a single run of the charset
/// including `.`): that looser reading would also "accept" a suffix built
/// entirely of dots (e.g. `..`, `-[dot][dot]`), since `.` is itself a member
/// of the charset -- it says nothing about a component between two dots
/// being non-empty. The identifier-list form is what the implementation
/// above actually enforces (`is_valid_semver_identifiers` splits on `.` and
/// rejects any empty piece), and it is what excludes STO-76's traversal
/// payload smuggled inside a suffix (e.g. `1.0.0-../..`).
pub fn is_plausible_release_tag(s: &str) -> bool {
    let (rest, build) = match s.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (s, None),
    };
    let (base, prerelease) = match rest.split_once('-') {
        Some((base, pre)) => (base, Some(pre)),
        None => (rest, None),
    };
    if !is_plausible_version(base) {
        return false;
    }
    if let Some(pre) = prerelease
        && !is_valid_semver_identifiers(pre)
    {
        return false;
    }
    if let Some(build) = build
        && !is_valid_semver_identifiers(build)
    {
        return false;
    }
    true
}

/// Whether `s` is a valid dot-separated run of semver identifiers: every
/// component (as split by `.`) must be non-empty and consist solely of ASCII
/// alphanumerics and `-`. An empty component (from a leading/trailing/doubled
/// `.`, as in `..`) is rejected, and so is any other character -- in
/// particular `/`, `\`, whitespace, and control characters -- wherever it
/// appears, even inside a single dot-free component. Used by
/// [`is_plausible_release_tag`] to validate the prerelease and build-metadata
/// suffixes.
fn is_valid_semver_identifiers(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Validate that `val` is a well-formed dotted numeric version string, as
/// required for `min-mind-version` (DSC-40). A valid string is non-empty, and
/// every dot-separated component is itself non-empty and consists solely of
/// ASCII decimal digits (e.g. `"1"`, `"0.7"`, `"2.3.1"`). Returns a
/// `MindToml` error naming `field` and the bad value when validation fails.
///
/// This validator applies to the *field value* in `mind.toml`. The running
/// binary's version string (from the build environment) may carry a
/// pre-release segment (e.g. `0.2.0-rc1`); such a segment is stripped from
/// the whole string (not per dotted component) before parsing in
/// [`version_at_least`], so `0.2.0-rc1` compares as `0.2.0`, not `0`.
fn validate_version_string(val: &str, field: &str, path: &Path) -> Result<()> {
    let bad = |reason: &str| {
        Err(MindError::MindToml {
            path: path.to_path_buf(),
            msg: format!(
                "field '{field}' must be a dotted numeric version \
                 (e.g. \"1\", \"0.7\", \"2.3.1\"); got {val:?} ({reason})"
            ),
        })
    };
    if val.is_empty() {
        return bad("empty string");
    }
    for component in val.split('.') {
        if component.is_empty() {
            return bad("empty component");
        }
        if !component.bytes().all(|b| b.is_ascii_digit()) {
            return bad(&format!(
                "component {component:?} is not a non-negative integer"
            ));
        }
    }
    Ok(())
}

/// Whether `running` satisfies `>= required`, comparing dotted numeric version
/// components (a missing component counts as 0, so `0.2` == `0.2.0`). A
/// trailing prerelease (`-suffix`) or build (`+suffix`) tag is stripped from
/// the WHOLE string, before splitting on `.`, so `0.22.1-dev` compares as
/// `0.22.1`, not `0.22.0`; a component with no leading digits at all still
/// compares as 0.
///
/// The strip happens once, on the whole string, rather than per dotted
/// component: stripping per-component would mis-parse a dotted prerelease
/// like `1.0.0-rc.2` as `[1, 0, 0, 2]` (the `.2` surviving as a spurious
/// fourth component), which reads as numerically ABOVE the plain `1.0.0` it
/// should tie with. Whole-string stripping instead parses `1.0.0-rc.2` as
/// `[1, 0, 0]`, tying with `1.0.0` in both directions, which is what lets
/// `selfupdate::decision` treat a dotted prerelease as offering the same-numbered
/// release rather than silently reporting up to date.
pub fn version_at_least(running: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        let end = v.find(['-', '+']).unwrap_or(v.len());
        v[..end]
            .split('.')
            .map(|c| c.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let r = parse(running);
    let req = parse(required);
    for i in 0..r.len().max(req.len()) {
        let a = r.get(i).copied().unwrap_or(0);
        let b = req.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
}

/// The running binary's version without its pre-release or build suffix
/// (`0.31.0-dev` -> `0.31.0`), the form `init-source` writes as a fresh
/// `min-mind-version` (INIT-13) and the validator accepts (DSC-40).
pub fn running_release_version() -> &'static str {
    let v = env!("CARGO_PKG_VERSION");
    &v[..v.find(['-', '+']).unwrap_or(v.len())]
}

/// The `[source].min-mind-version` a source declares when the running binary
/// is older than it, else `None` (DSC-40).
pub fn unmet_min_version(mindfile: Option<&MindToml>) -> Option<&str> {
    mindfile
        .and_then(|m| m.source.min_mind_version.as_deref())
        .filter(|required| !version_at_least(env!("CARGO_PKG_VERSION"), required))
}

/// Pull a plausible `[source].min-mind-version` out of a `mind.toml` the
/// strict parse rejected (DSC-117). Lenient on purpose: the file may use keys
/// or tables this binary predates, which is the case worth explaining.
fn probe_min_version(text: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let v = table.get("source")?.get("min-mind-version")?.as_str()?;
    is_plausible_version(v).then(|| v.to_string())
}

/// The `[source].min-mind-version` declared by `text`, when the running binary
/// does not meet it (VER-41). Read leniently, since a release's file may use
/// keys this binary predates.
pub fn unmet_min_version_in_text(text: &str) -> Option<String> {
    probe_min_version(text)
        .filter(|required| !version_at_least(env!("CARGO_PKG_VERSION"), required))
}

/// A path with ANSI/control/bidi code points stripped, for a message that will
/// be printed (DSC-95). A clone directory carries source-controlled components
/// (an owner, a repo, an item directory name), so a path is not inherently
/// safe terminal output.
fn sanitized_path(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(crate::sanitize::strip_ansi(&path.display().to_string()))
}

/// Only the `[source]` table of a `mind.toml`, for reading release metadata
/// from a file that is not checked out (VER-3). The rest of the file is
/// validated when that tree is loaded for real.
#[derive(Deserialize)]
struct SourceOnly {
    #[serde(default)]
    source: SourceMeta,
}

/// What a default-branch `mind.toml` says about releases (VER-3): the tag
/// template, the yanked list (VER-46), and the range it recommends (VER-140).
#[derive(Debug, Default)]
pub struct ReleaseMeta {
    pub template: Option<Template>,
    pub yanked: Vec<Version>,
    pub recommended: Option<String>,
    /// The declared auto-update level (AUP-5).
    pub auto_update: Option<crate::auto_update::Level>,
}

impl ReleaseMeta {
    /// The release metadata `meta` declares. A malformed template, yanked
    /// entry, or auto-update level is a `MindToml` error; a `pin-version` that
    /// does not parse recommends nothing.
    pub fn from_source(meta: &SourceMeta, toml_path: &Path) -> Result<ReleaseMeta> {
        Ok(ReleaseMeta {
            template: meta.release_template(toml_path)?,
            yanked: meta.yanked_versions(toml_path)?,
            recommended: meta.recommended_range().map(|r| r.to_string()),
            auto_update: meta.auto_update_level(toml_path)?,
        })
    }
}

/// The release metadata declared by `text`, a `mind.toml` read from the
/// default branch (VER-3, VER-46, VER-140).
pub fn release_meta_from_text(text: &str, toml_path: &Path) -> Result<ReleaseMeta> {
    let parsed: SourceOnly = toml::from_str(text).map_err(|e| MindError::Toml {
        path: sanitized_path(toml_path),
        msg: crate::sanitize::strip_ansi(&e.to_string()),
    })?;
    ReleaseMeta::from_source(&parsed.source, toml_path)
}

impl MindToml {
    /// Load `mind.toml` from a repo root, returning `None` if absent.
    ///
    /// Validates `[source].min-mind-version` format at parse time (DSC-40):
    /// the field, when present, must be a dotted purely-numeric version string.
    pub fn load(root: &Path) -> Result<Option<MindToml>> {
        let file = root.join("mind.toml");
        // spec: DSC-91 -- size-capped read: refuses an oversized mind.toml with
        // MindError::MetadataTooLarge before allocating the whole file, instead
        // of the earlier unconditional `std::fs::read_to_string`.
        let text = match crate::error::read_capped_metadata(&file) {
            Ok(text) => text,
            Err(MindError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        // spec: DSC-95 -- same sanitizing as the item-manifest read above: the
        // `toml` crate's parse error quotes the offending input line, which is
        // source-controlled text, and a raw ESC byte in a TOML file is always a
        // parse error, so this snippet is always reachable from `meld`/`recall`.
        let mut parsed: MindToml = toml::from_str(&text).map_err(|e| {
            let msg = crate::sanitize::strip_ansi(&e.to_string());
            // spec: DSC-117 -- a file written for a newer mind likely fails
            // here on a key this binary predates; name the version gap and
            // the update rather than leaving a bare schema error.
            match probe_min_version(&text) {
                Some(required) if !version_at_least(env!("CARGO_PKG_VERSION"), &required) => {
                    MindError::MindTomlTooNew {
                        path: sanitized_path(&file),
                        required,
                        running: env!("CARGO_PKG_VERSION").to_string(),
                        msg,
                    }
                }
                _ => MindError::Toml {
                    path: sanitized_path(&file),
                    msg,
                },
            }
        })?;
        // spec: DSC-40 — validate format of min-mind-version at parse time.
        if let Some(v) = &parsed.source.min_mind_version {
            validate_version_string(v, "min-mind-version", &file)?;
        }
        // spec: VER-2 VER-46 -- a malformed release-tags template or yanked
        // entry fails the load, not the later resolution.
        parsed.source.release_template(&sanitized_path(&file))?;
        parsed.source.yanked_versions(&sanitized_path(&file))?;
        // spec: AUP-5
        parsed.source.auto_update_level(&sanitized_path(&file))?;
        // spec: NS-25 — a declared `[source].prefix` that is a reserved
        // item-kind word is rejected at load, before it can reach the
        // effective-prefix resolution.
        //
        // spec: DSC-112 -- the reserved list is append-only, so this refusal
        // can also strike a source that melded cleanly before the word was
        // reserved (`workflow`, once the kind shipped). `catalog::scan_source_at`
        // re-words the `ReservedPrefix` it gets from here into
        // `MeldedSourceReservedPrefix`, which names the source and `mind unmeld`;
        // this load site has neither the source identity nor the registry to
        // know it is looking at one.
        if let Some(p) = &parsed.source.prefix {
            crate::namespace::validate_prefix(p)?;
        }
        // spec: DSC-81 — validate [discover] glob patterns at parse time:
        // reject absolute patterns and patterns with `..` components before
        // they can reach `glob_paths` and the filesystem.
        if let Some(discover) = &parsed.discover {
            validate_discover_patterns(discover, &file)?;
        }
        // spec: PROF-2 PROF-3 PROF-4 -- the profile table's syntax, members,
        // and inclusion graph are checked here; an exact member's referent
        // needs the catalog and is checked at expansion (PROF-5).
        if let Some(profiles) = &parsed.profiles {
            crate::profile::validate_table(profiles, &sanitized_path(&file))?;
        }
        // spec: DSC-92 -- a curated `[discover].sources` entry's relative local
        // `source` (`./rel/path`, `../rel/path`) is resolved against the
        // directory THIS mind.toml was read from, not against whatever
        // directory the consumer running `meld`/`sync` happens to be in.
        // Fixed here (the one read site) rather than at each use site, so it
        // covers meld's curated-source walk, sync's re-walk, and dump
        // uniformly. `root` itself is absolutized first in case a caller
        // passed a relative root.
        if let Some(discover) = &mut parsed.discover {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let root_abs = std::path::PathBuf::from(crate::source::absolutize_against(
                &cwd,
                &root.to_string_lossy(),
            ));
            for ns in &mut discover.sources {
                if crate::source::is_relative_local_spec(&ns.source) {
                    ns.source = crate::source::absolutize_against(&root_abs, &ns.source);
                }
            }
        }
        Ok(Some(parsed))
    }

    /// Whether this file takes over item discovery (vs. leaving it to
    /// convention). Nested `[discover].sources` alone does not.
    pub fn is_authoritative(&self) -> bool {
        !self.items.is_empty() || self.discover.as_ref().is_some_and(|d| d.has_item_globs())
    }

    /// Whether the file relocates or reshapes convention discovery of the repo's
    /// OWN items via `[source].roots` or `[source].flat-skills` (as opposed to the
    /// authoritative `[[items]]`/`[discover]`-glob inventory, [`is_authoritative`]).
    ///
    /// This is the non-authoritative half of an "own-item source-discovery
    /// directive" (MKT-15): when set, the author is defining the repo's own items
    /// through convention at a chosen layout, which suppresses a co-present
    /// `.claude-plugin/` manifest's own-item layer (alongside the authoritative
    /// case, [`is_authoritative`]). A bare `[discover].sources` list is a curator
    /// directive over OTHER repos, not an own-item directive, so it deliberately
    /// does not count here (MKT-16).
    pub fn declares_scan_layout(&self) -> bool {
        // spec: MKT-15
        self.source.roots.is_some() || self.source.flat_skills
    }

    /// Return all hooks in resolved, normalized form.
    ///
    /// The legacy `[source].install` (HOOK-50 back-compat) folds in as the
    /// first required install hook when non-empty after trimming. Then each
    /// `[[hooks]]` entry is validated and appended in declaration order.
    /// Entries with an empty/whitespace `run` are silently dropped (HOOK-3).
    pub fn resolved_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        let mut out: Vec<ResolvedHook> = Vec::new();

        // HOOK-50: fold legacy [source].install into a required install hook.
        if let Some(cmd) = &self.source.install {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                out.push(ResolvedHook {
                    run: trimmed.to_owned(),
                    name: None,
                    optional: false,
                    event: HookEvent::Install,
                });
            }
        }

        // HOOK-51: [[hooks]] entries in declaration order.
        out.extend(resolve_hook_array(&self.hooks, toml_path, "[source]")?);

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn root_mindfile_curates_skill_libraries_register_only() {
        // The repo-root mind.toml is the source behind `mind meld jaemk/mind`. It
        // curates two external skill libraries as register-only nested sources
        // (DSC-58 default install = false) while staying non-authoritative so its
        // own convention discovery still surfaces the hello-mind example (DSC-35).
        // This validates the real file offline; the live recursive meld is network
        // dependent and is guarded hermetically with local stand-ins in tests/cli.rs.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mt = MindToml::load(root)
            .expect("root mind.toml must parse")
            .expect("root mind.toml must exist");
        assert!(
            !mt.is_authoritative(),
            "a bare [discover].sources list must keep convention discovery (DSC-35)"
        );
        let sources = &mt
            .discover
            .as_ref()
            .expect("must declare [discover]")
            .sources;
        let urls: Vec<&str> = sources.iter().map(|s| s.source.as_str()).collect();
        assert!(
            urls.contains(&"https://github.com/anthropics/skills"),
            "must curate anthropics/skills: {urls:?}"
        );
        assert!(
            urls.contains(&"https://github.com/ComposioHQ/awesome-claude-skills"),
            "must curate ComposioHQ/awesome-claude-skills: {urls:?}"
        );
        for s in sources {
            assert!(
                !s.install,
                "curated source '{}' must be register-only (install = false)",
                s.source
            );
            assert!(
                s.install_items.is_none(),
                "curated source '{}' must not pin an install subset",
                s.source
            );
        }
    }

    #[test]
    fn declares_scan_layout_covers_roots_and_flat_skills_but_not_sources() {
        // spec: MKT-15, MKT-16
        // `declares_scan_layout` is the non-authoritative own-item directive that
        // suppresses a co-present .claude-plugin/ manifest: `[source].roots` or
        // `[source].flat-skills`. A bare `[discover].sources` list (MKT-16) and a
        // `[source]`-metadata-only file are NOT own-item directives.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        // (mind.toml body, declares_scan_layout, is_authoritative)
        let cases: &[(&str, bool, bool)] = &[
            // roots-only: a scan layout, not authoritative.
            ("[source]\nroots = [\"pkg\"]\n", true, false),
            // explicitly empty roots is still a declared layout (DSC-50).
            ("[source]\nroots = []\n", true, false),
            // flat-skills only: a scan layout, not authoritative.
            ("[source]\nflat-skills = true\n", true, false),
            // flat-skills = false is NOT a declared layout.
            ("[source]\nflat-skills = false\n", false, false),
            // bare [discover].sources: a curator directive, NOT an own-item one.
            (
                "[source]\n[discover]\nsources = [{ source = \"owner/repo\" }]\n",
                false,
                false,
            ),
            // metadata-only: nothing declared.
            ("[source]\ndescription = \"x\"\n", false, false),
            // authoritative [[items]] is a separate arm; not a scan layout.
            (
                "[[items]]\nkind = \"rule\"\nname = \"style\"\npath = \"r.md\"\n",
                false,
                true,
            ),
            // roots alongside a curator [discover].sources: still a scan layout.
            (
                "[source]\nroots = [\"pkg\"]\n[discover]\nsources = [{ source = \"owner/repo\" }]\n",
                true,
                false,
            ),
        ];
        for (body, want_layout, want_auth) in cases {
            let n = N.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("mind-mkt15-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("mind.toml"), body).unwrap();
            let mt = MindToml::load(&dir)
                .expect("must parse")
                .expect("must exist");
            assert_eq!(
                mt.declares_scan_layout(),
                *want_layout,
                "declares_scan_layout mismatch for body: {body:?}"
            );
            assert_eq!(
                mt.is_authoritative(),
                *want_auth,
                "is_authoritative mismatch for body: {body:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // spec: DSC-92
    #[test]
    fn nested_relative_local_source_resolves_against_the_mind_toml_directory_not_cwd() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-dsc92-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let curator = base.join("curator");
        let sibling = base.join("explicit");
        std::fs::create_dir_all(&curator).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(
            curator.join("mind.toml"),
            "[discover]\nsources = [{ source = \"../explicit\" }]\n",
        )
        .unwrap();

        let mt = MindToml::load(&curator)
            .expect("must parse")
            .expect("file exists");
        let ns = &mt
            .discover
            .as_ref()
            .expect("must declare [discover]")
            .sources[0];
        assert_eq!(
            ns.source,
            sibling.to_string_lossy(),
            "the relative nested source must resolve against the mind.toml's own directory \
             ({curator:?}), not the test process's cwd"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: DSC-92
    #[test]
    fn nested_absolute_or_remote_source_is_left_unchanged() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-dsc92-noop-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mind.toml"),
            "[discover]\nsources = [\
             { source = \"owner/repo\" }, \
             { source = \"/already/absolute\" }\
             ]\n",
        )
        .unwrap();

        let mt = MindToml::load(&dir)
            .expect("must parse")
            .expect("file exists");
        let sources = &mt
            .discover
            .as_ref()
            .expect("must declare [discover]")
            .sources;
        assert_eq!(
            sources[0].source, "owner/repo",
            "a remote spec is untouched"
        );
        assert_eq!(
            sources[1].source, "/already/absolute",
            "an already-absolute local path is untouched"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: DSC-92
    // The `./rel/path` leg (only `../explicit` was driven), EVERY entry in the
    // list (not just the first), and the fields the rewrite must leave alone.
    // A loop that rewrote only `sources[0]` would pass the existing coverage.
    #[test]
    fn every_relative_nested_source_is_rewritten_and_sibling_fields_are_preserved() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-dsc92-many-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let curator = base.join("curator");
        std::fs::create_dir_all(&curator).unwrap();
        std::fs::write(
            curator.join("mind.toml"),
            "[discover]\nsources = [\
             { source = \"owner/repo\" }, \
             { source = \"./inner/one\", namespace = \"one\", install = true }, \
             { source = \"../sibling/two\" }\
             ]\n",
        )
        .unwrap();

        let mt = MindToml::load(&curator)
            .expect("must parse")
            .expect("file exists");
        let sources = &mt
            .discover
            .as_ref()
            .expect("must declare [discover]")
            .sources;

        assert_eq!(sources[0].source, "owner/repo", "remote entry untouched");
        assert_eq!(
            sources[1].source,
            curator.join("inner/one").to_string_lossy(),
            "a './' entry resolves against the mind.toml's directory"
        );
        assert_eq!(
            sources[2].source,
            base.join("sibling/two").to_string_lossy(),
            "a later '../' entry must be rewritten too, not just the first entry"
        );
        // The rewrite touches `source` only.
        assert_eq!(sources[1].namespace.as_deref(), Some("one"));
        assert!(sources[1].install, "install flag must survive the rewrite");

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: DSC-92
    // The rewrite is purely lexical (it mirrors STO-72's stat-free resolution),
    // so it applies even when the target does not exist yet. Gating it on
    // existence would silently leave a cwd-relative spec in place for a nested
    // source that is simply not checked out yet, which is the same bug in a
    // quieter form: the failure would then name a path relative to wherever the
    // consumer stood.
    #[test]
    fn a_relative_nested_source_is_rewritten_even_when_the_target_does_not_exist() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let curator =
            std::env::temp_dir().join(format!("mind-dsc92-ghost-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&curator);
        std::fs::create_dir_all(&curator).unwrap();
        std::fs::write(
            curator.join("mind.toml"),
            "[discover]\nsources = [{ source = \"./never-created\" }]\n",
        )
        .unwrap();

        let mt = MindToml::load(&curator)
            .expect("must parse")
            .expect("file exists");
        assert_eq!(
            mt.discover.as_ref().unwrap().sources[0].source,
            curator.join("never-created").to_string_lossy(),
            "the rewrite must not be gated on the target existing"
        );

        let _ = std::fs::remove_dir_all(&curator);
    }

    #[test]
    fn version_comparison_orders_dotted_components() {
        // spec: DSC-40
        assert!(version_at_least("0.2.0", "0.2"));
        assert!(version_at_least("0.2", "0.2.0"));
        assert!(version_at_least("1.0.0", "0.9.9"));
        assert!(version_at_least("0.10.0", "0.9.0"));
        assert!(!version_at_least("0.1.0", "0.2"));
        assert!(!version_at_least("0.1.0", "0.1.1"));
        // Non-numeric / suffix components in the running binary's version count
        // as 0; validation of the field value is a separate concern.
        assert!(version_at_least("0.2.0-rc1", "0.2"));
    }

    #[test]
    fn version_comparison_strips_prerelease_and_build_suffix_before_parsing() {
        // spec: DSC-40, POL-61 -- a `-dev`/`-rcN`/`+build` tag on the final
        // dotted component must not zero out its leading digits (H1): a dev
        // build of a not-yet-released patch satisfies a min-version gate for
        // that patch, and an actually older version is still refused.
        assert!(version_at_least("0.22.1-dev", "0.22.1"));
        assert!(version_at_least("0.23.1-dev", "0.23.1"));
        assert!(version_at_least("0.22.1+build.5", "0.22.1"));
        assert!(version_at_least("0.22.1-dev", "0.22.0"));
        assert!(!version_at_least("0.22.1-dev", "0.22.2"));
        // A real older running version is still refused even though its own
        // suffix parses fine -- the fix must not make the gate permissive.
        assert!(!version_at_least("0.21.9-dev", "0.22.0"));
    }

    #[test]
    // spec: DSC-40
    fn version_comparison_strips_a_dotted_prerelease_suffix_from_the_whole_string() {
        // A DOTTED prerelease suffix (e.g. semver's `-rc.2`) must be stripped
        // from the whole string before splitting on '.', not per dotted
        // component. Per-component stripping mis-parses "1.0.0-rc.2" as
        // [1, 0, 0, 2] (the ".2" surviving as a spurious 4th component),
        // which reads as numerically ABOVE the plain "1.0.0" it should tie
        // with -- so the "required" side (1.0.0) fails `>= "1.0.0-rc.2"`.
        // With whole-string stripping both sides parse as [1, 0, 0] and tie.
        assert!(
            version_at_least("1.0.0", "1.0.0-rc.2"),
            "1.0.0 must satisfy >= 1.0.0-rc.2 (both parse as [1,0,0]); \
             per-component stripping would wrongly refuse this"
        );
        assert!(version_at_least("1.0.0-rc.2", "1.0.0"));
        // A dotted prerelease of a genuinely lower version is still refused.
        assert!(!version_at_least("1.0.0-rc.2", "1.0.1"));
        assert!(!version_at_least("0.9.0-rc.2", "1.0.0"));
    }

    #[test]
    // spec: STO-76
    fn is_plausible_version_unaffected_by_the_release_tag_sibling() {
        // is_plausible_version keeps answering its ORIGINAL question (a valid
        // version pin / min-mind-version): digits and dots only, no
        // prerelease/build suffix. Adding is_plausible_release_tag alongside
        // it must not change this behavior at all -- policy.rs's two call
        // sites (version pin validation) depend on it staying exactly this
        // strict.
        for v in ["1", "0.7", "2.3.1", "10.0.1"] {
            assert!(is_plausible_version(v), "{v:?} must still be plausible");
        }
        for v in ["1.2.3-rc1", "1.2.3+build", "", "1..0", "1.x", "abc", "1/2"] {
            assert!(
                !is_plausible_version(v),
                "{v:?} must still be rejected by is_plausible_version"
            );
        }
    }

    #[test]
    // spec: STO-76
    fn is_plausible_release_tag_accepts_dotted_numeric_and_semver_prerelease_build() {
        // The base dotted-numeric shape (identical to is_plausible_version)
        // and a semver-shaped prerelease and/or build suffix are all accepted
        // -- this is what lets `evolve --to 1.2.3-rc1` reach an rc release,
        // which GitHub's `releases/latest` never surfaces.
        for v in [
            "1",
            "0.7",
            "2.3.1",
            "1.2.3-rc1",
            "1.2.3-rc.2",
            "1.2.3-alpha-1",
            "1.2.3+build.5",
            "1.2.3-rc1+build.5",
            "1.2.3-0",
        ] {
            assert!(is_plausible_release_tag(v), "{v:?} must be accepted");
        }
    }

    #[test]
    // spec: STO-76
    fn is_plausible_release_tag_rejects_path_segments_and_traversal() {
        // A bare path-traversal payload (no dash) must still be rejected,
        // exactly as before this validator existed.
        assert!(!is_plausible_release_tag(
            "1/../../../../attacker/mind/releases/download/v1"
        ));
        assert!(!is_plausible_release_tag("../etc/passwd"));
        assert!(!is_plausible_release_tag("1.0.0/evil"));
        // A traversal payload smuggled INSIDE the prerelease suffix must be
        // rejected too: a `-`-prefixed suffix of dots and slashes is not
        // meaningfully different from a bare traversal payload, so the dash
        // must not become an escape hatch.
        assert!(!is_plausible_release_tag("1.0.0-../.."));
        assert!(!is_plausible_release_tag("1.0.0-rc1/../.."));
        assert!(!is_plausible_release_tag("1.0.0-rc1/etc/passwd"));
        // Smuggled inside the build-metadata suffix too.
        assert!(!is_plausible_release_tag("1.0.0+../.."));
        assert!(!is_plausible_release_tag("1.0.0+build/../.."));
        // Backslash, whitespace, and control characters must be rejected
        // wherever they appear, not only as a bare `/`.
        assert!(!is_plausible_release_tag("1.0.0-rc1\\..\\.."));
        assert!(!is_plausible_release_tag("1.0.0-rc 1"));
        assert!(!is_plausible_release_tag("1.0.0-rc\n1"));
        assert!(!is_plausible_release_tag("1.0.0-rc\x001"));
        // Empty, and a base that itself is not dotted-numeric.
        assert!(!is_plausible_release_tag(""));
        assert!(!is_plausible_release_tag("-rc1"));
        assert!(!is_plausible_release_tag("v1.2.3"));
        assert!(!is_plausible_release_tag("1.x.0"));
    }

    #[test]
    // spec: STO-76
    fn is_plausible_release_tag_rejects_further_boundary_cases() {
        // L7: boundary cases the doc grammar implies but the existing test
        // suite did not exercise. Each behaves correctly today; pinned here
        // so a future rewrite of the split-based validator cannot silently
        // change the ordering/behavior at these edges.

        // A pure dot-run suffix with NO slash: under a naive reading of the
        // OLD doc comment's bare character-class grammar (`.` is a member of
        // `[0-9A-Za-z.-]`), a suffix of only dots would look "in charset" and
        // could seem plausible. The real implementation splits on `.` and
        // rejects any EMPTY identifier, so a run of two (or more) consecutive
        // dots -- with no other characters between them -- is still refused.
        // This is the precise grammar gap the corrected doc comment above
        // calls out: it is the difference between "every char is in the
        // charset" and "every dot-separated identifier is non-empty".
        assert!(!is_plausible_release_tag("1.0.0-.."));
        assert!(!is_plausible_release_tag("1.0.0+.."));

        // A trailing `-` (an empty prerelease identifier: split_once yields
        // prerelease = "").
        assert!(!is_plausible_release_tag("1.2.3-"));
        // A trailing `+` (an empty build identifier).
        assert!(!is_plausible_release_tag("1.2.3+"));
        // A SECOND `+`: split_once('+') takes the FIRST '+', so the build
        // identifier becomes "a+b" (embedding a literal '+', which is
        // outside the semver-identifier charset) -- not "a" with a silently
        // dropped "+b" tail.
        assert!(!is_plausible_release_tag("1.2.3+a+b"));
        assert!(
            is_plausible_release_tag("1.2.3+a"),
            "sanity: a clean single build identifier is accepted"
        );

        // A multi-byte (non-ASCII) component inside the prerelease suffix:
        // `is_valid_semver_identifiers` checks `b.is_ascii_alphanumeric()`
        // per BYTE, so a UTF-8 multi-byte character (whose continuation
        // bytes are never ASCII-alphanumeric) is rejected.
        assert!(!is_plausible_release_tag("1.2.3-rc\u{e9}"));
    }

    #[test]
    fn min_mind_version_well_formed_parses_ok() {
        // spec: DSC-40 -- well-formed dotted numeric versions are accepted.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        for v in ["0.7", "2", "1.2.3", "0", "10.0.1"] {
            let n = N.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("mind-dsc40-ok-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("mind.toml"),
                format!("[source]\nmin-mind-version = \"{v}\"\n"),
            )
            .unwrap();
            let result = MindToml::load(&dir);
            assert!(
                result.is_ok(),
                "min-mind-version = {v:?} must parse OK, got: {result:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn unparseable_mind_toml_names_an_unmet_min_version() {
        // spec: DSC-117 -- a key this binary predates plus a newer
        // min-mind-version reads as "update mind", not a bare schema error.
        let load = |name: &str, body: &str| {
            let dir =
                std::env::temp_dir().join(format!("mind-dsc117-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("mind.toml"), body).unwrap();
            let r = MindToml::load(&dir);
            let _ = std::fs::remove_dir_all(&dir);
            r
        };
        let err = load(
            "new",
            "[source]\nmin-mind-version = \"99.0\"\nfuture-key = true\n",
        )
        .unwrap_err();
        match &err {
            MindError::MindTomlTooNew { required, msg, .. } => {
                assert_eq!(required, "99.0");
                assert!(msg.contains("future-key"), "{msg}");
            }
            other => panic!("expected MindTomlTooNew, got {other:?}"),
        }
        assert!(err.to_string().contains("mind evolve"), "{err}");

        // A satisfiable floor keeps the plain schema error: the typo is the
        // author's, not a version gap.
        let err = load(
            "old",
            "[source]\nmin-mind-version = \"0.0.1\"\nfuture-key = true\n",
        )
        .unwrap_err();
        assert!(matches!(err, MindError::Toml { .. }), "{err:?}");
        // No declared floor: plain schema error too.
        let err = load("none", "[source]\nfuture-key = true\n").unwrap_err();
        assert!(matches!(err, MindError::Toml { .. }), "{err:?}");
    }

    #[test]
    fn unmet_min_version_compares_against_the_running_binary() {
        // spec: DSC-40
        let with = |v: &str| -> MindToml {
            toml::from_str(&format!("[source]\nmin-mind-version = \"{v}\"\n")).unwrap()
        };
        assert_eq!(unmet_min_version(Some(&with("99.0"))), Some("99.0"));
        assert_eq!(unmet_min_version(Some(&with("0.0.1"))), None);
        assert_eq!(
            unmet_min_version(Some(&with(running_release_version()))),
            None
        );
        assert_eq!(unmet_min_version(None), None);
    }

    #[test]
    fn min_mind_version_malformed_is_mind_toml_error() {
        // spec: DSC-40 -- malformed values are rejected with MindToml naming the
        // field: non-numeric component, empty string, embedded suffix like `-beta`.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        for v in ["0.3-beta", "abc", "", "1.x", "0.", ".1", "1..0"] {
            let n = N.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("mind-dsc40-bad-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("mind.toml"),
                format!("[source]\nmin-mind-version = \"{v}\"\n"),
            )
            .unwrap();
            let err = MindToml::load(&dir).unwrap_err();
            match err {
                MindError::MindToml { msg, .. } => {
                    assert!(
                        msg.contains("min-mind-version"),
                        "error must name the field for {v:?}: {msg}"
                    );
                    // The bad value should appear in the message (if non-empty).
                    if !v.is_empty() {
                        assert!(
                            msg.contains(v),
                            "error must include the bad value {v:?}: {msg}"
                        );
                    }
                }
                other => panic!("expected MindError::MindToml for {v:?}, got: {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn declared_reserved_kind_prefix_is_rejected_on_load() {
        // spec: NS-25 CMD-9 — a `[source].prefix` equal to a reserved item-kind
        // word (skill/agent/rule/command/tool) is rejected at load with
        // ReservedPrefix, before it can become an effective prefix that would
        // alias `prefix:name` onto a kind-qualified ref. `command` is a kind
        // word like the rest (CMD-9), so it is reserved on the same terms.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        for word in ["skill", "agent", "rule", "command", "tool"] {
            let n = N.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("mind-ns25-bad-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("mind.toml"),
                format!("[source]\nprefix = \"{word}\"\n"),
            )
            .unwrap();
            let err = MindToml::load(&dir).unwrap_err();
            assert!(
                matches!(err, MindError::ReservedPrefix { ref prefix } if prefix == word),
                "expected ReservedPrefix for declared prefix {word:?}, got: {err:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn declared_normal_prefix_loads_ok() {
        // spec: NS-25 — a non-reserved declared prefix loads fine.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-ns25-ok-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), "[source]\nprefix = \"jk\"\n").unwrap();
        let mt = MindToml::load(&dir)
            .expect("normal prefix must load")
            .expect("file exists");
        assert_eq!(mt.source.prefix.as_deref(), Some("jk"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- DSC-91: capped mind.toml read --------------------------------------

    #[test]
    fn normal_sized_mind_toml_is_unaffected_by_the_cap() {
        // spec: DSC-91
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-dsc91-ok-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mind.toml"),
            "[source]\ndescription = \"a perfectly normal source\"\n",
        )
        .unwrap();
        let mt = MindToml::load(&dir)
            .expect("normal-sized mind.toml must load fine")
            .expect("file exists");
        assert_eq!(
            mt.source.description.as_deref(),
            Some("a perfectly normal source")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_mind_toml_is_refused_naming_the_file() {
        // spec: DSC-91
        // A mind.toml at (limit + 1) bytes is refused with MetadataTooLarge
        // naming the file, instead of being parsed (and failing later with a
        // confusing Toml parse error) or silently truncated. Built as a sparse
        // file so the test does not itself allocate the whole oversized buffer.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-dsc91-big-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let toml_path = dir.join("mind.toml");
        let file = std::fs::File::create(&toml_path).unwrap();
        file.set_len(crate::error::METADATA_SIZE_LIMIT + 1).unwrap();
        drop(file);
        let err = MindToml::load(&dir).expect_err("oversized mind.toml must be refused");
        match &err {
            MindError::MetadataTooLarge { path, .. } => {
                assert_eq!(path, &toml_path, "error must name the offending file");
            }
            other => panic!("expected MetadataTooLarge, got: {other:?}"),
        }
        assert!(
            err.to_string().contains(&toml_path.display().to_string()),
            "error message must name the file: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_install_hook_parses() {
        // spec: HOOK-1
        let toml = r#"
            [source]
            description = "tools"
            install = "make build && make install"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert_eq!(
            parsed.source.install.as_deref(),
            Some("make build && make install")
        );

        // Absent install => None.
        let none: MindToml = toml::from_str("[source]\ndescription = \"x\"\n").unwrap();
        assert_eq!(none.source.install, None);
    }

    #[test]
    fn pin_directive_none_when_no_fields_set() {
        // spec: DSC-41
        let meta = SourceMeta::default();
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("should not error");
        assert!(pin.is_none(), "no directive set => None");
    }

    #[test]
    fn pin_directive_follow_branch() {
        // spec: DSC-41
        let meta = SourceMeta {
            follow_branch: Some("develop".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::FollowBranch("develop".into())));
    }

    #[test]
    fn pin_directive_tag() {
        // spec: DSC-41
        let meta = SourceMeta {
            pin_tag: Some("v2.0".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::Tag("v2.0".into())));
    }

    #[test]
    fn pin_directive_ref() {
        // spec: DSC-41
        let meta = SourceMeta {
            pin_ref: Some("abc123".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::Ref("abc123".into())));
    }

    #[test]
    fn pin_directive_version() {
        // spec: VER-51
        let meta = SourceMeta {
            pin_version: Some("^1.2".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        match pin {
            Some(Pin::Version(v)) => {
                assert_eq!(v.range, "^1.2");
                assert!(
                    v.resolved.is_none(),
                    "resolved at meld time, not parse time"
                );
            }
            other => panic!("expected a version pin, got {other:?}"),
        }
    }

    #[test]
    fn an_exact_pin_version_reads_as_its_caret() {
        // spec: VER-53
        let exact = SourceMeta {
            pin_version: Some("=1.2.3".into()),
            ..Default::default()
        };
        assert!(exact.pin_version_is_exact());
        match exact
            .pin_directive(Path::new("mind.toml"))
            .expect("no error")
        {
            Some(Pin::Version(v)) => assert_eq!(v.range, "^1.2.3"),
            other => panic!("expected a version pin, got {other:?}"),
        }
        assert_eq!(exact.recommended_range().unwrap().to_string(), "^1.2.3");

        let caret = SourceMeta {
            pin_version: Some("^1.2".into()),
            ..Default::default()
        };
        assert!(!caret.pin_version_is_exact());
        assert_eq!(caret.recommended_range().unwrap().to_string(), "^1.2");
    }

    #[test]
    fn pin_directive_version_must_parse_and_is_one_of() {
        // spec: VER-51
        let bad = SourceMeta {
            pin_version: Some("v1".into()),
            ..Default::default()
        };
        let err = bad.pin_directive(Path::new("/repo/mind.toml")).unwrap_err();
        assert!(matches!(err, MindError::MindToml { .. }), "{err:?}");
        assert!(err.to_string().contains("pin-version"), "{err}");

        let both = SourceMeta {
            pin_version: Some("^1".into()),
            follow_branch: Some("main".into()),
            ..Default::default()
        };
        let err = both
            .pin_directive(Path::new("/repo/mind.toml"))
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("follow-branch, pin-version"),
            "both keys are named: {msg}"
        );
    }

    #[test]
    fn pin_directive_conflict_is_an_error() {
        // spec: DSC-41 — more than one directive is a MindToml error
        let meta = SourceMeta {
            follow_branch: Some("main".into()),
            pin_tag: Some("v1.0".into()),
            ..Default::default()
        };
        let result = meta.pin_directive(Path::new("/repo/mind.toml"));
        assert!(result.is_err(), "conflicting directives must error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("conflicting pin"),
            "expected 'conflicting pin' in: {err_msg}"
        );
    }

    // ----- HOOK-51: [[hooks]] multi-hook schema -----

    #[test]
    fn hooks_parse_fields_correctly() {
        // spec: HOOK-51
        let toml = r#"
            [[hooks]]
            run = "make build"
            name = "Build step"
            optional = true
            event = "install"

            [[hooks]]
            run = "make clean"
            event = "uninstall"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert_eq!(parsed.hooks.len(), 2);

        let h0 = &parsed.hooks[0];
        assert_eq!(h0.run, "make build");
        assert_eq!(h0.name.as_deref(), Some("Build step"));
        assert!(h0.optional);
        assert_eq!(h0.event.as_deref(), Some("install"));

        let h1 = &parsed.hooks[1];
        assert_eq!(h1.run, "make clean");
        assert_eq!(h1.name, None);
        assert!(!h1.optional);
        assert_eq!(h1.event.as_deref(), Some("uninstall"));
    }

    #[test]
    fn hooks_preserve_declaration_order() {
        // spec: HOOK-51
        let toml = r#"
            [[hooks]]
            run = "first"

            [[hooks]]
            run = "second"

            [[hooks]]
            run = "third"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].run, "first");
        assert_eq!(resolved[1].run, "second");
        assert_eq!(resolved[2].run, "third");
    }

    #[test]
    fn legacy_install_folds_in_as_first_required_install_hook() {
        // spec: HOOK-50
        let toml = r#"
            [source]
            install = "make legacy"

            [[hooks]]
            run = "npm install"
            event = "install"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 2);

        // Legacy hook is first.
        assert_eq!(resolved[0].run, "make legacy");
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert!(!resolved[0].optional);
        assert_eq!(resolved[0].name, None);

        // Declared hook follows.
        assert_eq!(resolved[1].run, "npm install");
        assert_eq!(resolved[1].event, HookEvent::Install);
    }

    #[test]
    fn legacy_install_only_no_hooks_table() {
        // spec: HOOK-50 — install field alone, no [[hooks]] section
        let toml = r#"
            [source]
            install = "make build && make install"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "make build && make install");
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert!(!resolved[0].optional);
    }

    #[test]
    fn a_commands_only_discover_is_authoritative_and_validated() {
        // spec: CMD-4 DSC-3 DSC-81
        // Every per-kind glob list must reach both predicates: a `[discover]`
        // that names only commands turns off convention scanning like any other
        // item globs, and its patterns are confinement-checked at load.
        let toml = r#"
            [discover]
            commands = { include = ["packages/*/commands/*.md"] }
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert!(
            parsed.is_authoritative(),
            "commands globs are item globs, so they make the file authoritative"
        );

        for bad in ["/etc/commands/*.md", "../outside/commands/*.md"] {
            let text = format!("[discover]\ncommands = {{ include = [\"{bad}\"] }}\n");
            let parsed: MindToml = toml::from_str(&text).expect("parse");
            let err = validate_discover_patterns(
                parsed.discover.as_ref().expect("discover"),
                Path::new("mind.toml"),
            )
            .expect_err("an unconfined commands glob must be rejected at load");
            assert!(
                err.to_string().contains("discover glob"),
                "the rejection must name the glob: {err}"
            );
        }
    }

    #[test]
    fn update_event_resolves_and_is_its_own_event() {
        // spec: HOOK-120
        let toml = r#"
            [[hooks]]
            run = "make install"

            [[hooks]]
            run = "make upgrade"
            name = "Upgrade"
            event = "update"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let resolved = parsed
            .resolved_hooks(Path::new("mind.toml"))
            .expect("resolve");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert_eq!(resolved[1].event, HookEvent::Update);
        assert_eq!(resolved[1].run, "make upgrade");
        assert_eq!(resolved[1].label(), "Upgrade");
        assert_eq!(HookEvent::Update.as_str(), "update");
    }

    #[test]
    fn unknown_event_names_the_legal_set_including_update() {
        // spec: HOOK-120 HOOK-51
        let toml = r#"
            [[hooks]]
            run = "make build"
            event = "upgrade"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let err = parsed
            .resolved_hooks(Path::new("mind.toml"))
            .expect_err("an unknown event is a schema error")
            .to_string();
        assert!(err.contains("'upgrade'"), "must name the bad value: {err}");
        assert!(
            err.contains("'install', 'update', or 'uninstall'"),
            "must name the legal set: {err}"
        );
    }

    #[test]
    fn item_scalar_update_folds_in_between_install_and_uninstall() {
        // spec: HOOK-120 HOOK-125
        let decl = ItemDecl {
            kind: "skill".into(),
            name: "scanner".into(),
            path: "skills/scanner".into(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: Some("setup.sh".into()),
            update: Some("migrate.sh".into()),
            uninstall: Some("teardown.sh".into()),
            hooks: Vec::new(),
            ignore: None,
        };
        let resolved = decl
            .resolved_item_hooks(Path::new("mind.toml"))
            .expect("resolve");
        let events: Vec<HookEvent> = resolved.iter().map(|h| h.event).collect();
        assert_eq!(
            events,
            vec![HookEvent::Install, HookEvent::Update, HookEvent::Uninstall]
        );
        assert_eq!(resolved[1].run, "migrate.sh");
    }

    #[test]
    fn declares_hooks_is_true_only_for_a_non_blank_declaration() {
        // spec: HOOK-132
        let bare = |install: Option<&str>, hooks: Vec<Hook>| ItemDecl {
            kind: "skill".into(),
            name: "scanner".into(),
            path: "skills/scanner".into(),
            link: None,
            description: None,
            bin: None,
            build: None,
            install: install.map(str::to_string),
            update: None,
            uninstall: None,
            hooks,
            ignore: None,
        };
        assert!(
            !bare(None, Vec::new()).declares_hooks(),
            "a declaration that says nothing about hooks leaves the item free to \
             declare its own"
        );
        assert!(
            !bare(Some("   "), Vec::new()).declares_hooks(),
            "a blank command is absent (HOOK-3), so it declares nothing"
        );
        assert!(bare(Some("setup.sh"), Vec::new()).declares_hooks());
        assert!(
            bare(
                None,
                vec![Hook {
                    run: "setup.sh".into(),
                    name: None,
                    optional: false,
                    event: None,
                }],
            )
            .declares_hooks()
        );
    }

    #[test]
    fn item_manifest_reads_hooks_and_rejects_source_keys() {
        // spec: HOOK-131
        let dir =
            std::env::temp_dir().join(format!("mind-item-toml-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("mind.toml"),
            "[[hooks]]\nrun = \"setup.sh\"\n\n[[hooks]]\nrun = \"migrate.sh\"\nevent = \"update\"\n",
        )
        .unwrap();
        let parsed = ItemToml::load(&dir)
            .expect("an item manifest parses")
            .expect("present");
        let hooks = parsed
            .resolved_hooks(&dir.join("mind.toml"))
            .expect("resolve");
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].event, HookEvent::Install);
        assert_eq!(hooks[1].event, HookEvent::Update);

        // A root manifest's own tables are not part of the item schema.
        let manifest = dir.join("mind.toml");
        std::fs::write(
            &manifest,
            "[source]\ndescription = \"nope\"\n\n[[hooks]]\nrun = \"setup.sh\"\n",
        )
        .unwrap();
        let err = ItemToml::load(&dir).expect_err("a source key must be rejected");
        assert!(
            err.to_string().contains("source"),
            "the error must name the offending key: {err}"
        );
        // HOOK-131 requires the error to NAME THE FILE: the serde text alone
        // says only that a key is unknown, which leaves an author who has
        // several item manifests with nothing to open.
        assert!(
            err.to_string().contains(&manifest.display().to_string()),
            "the error must name the manifest it failed to read: {err}"
        );
        // The other half of the schema: an `[[items]]` inventory (the most
        // likely paste, since it is what a source manifest is mostly made of)
        // is rejected the same way, not half-read.
        std::fs::write(
            &manifest,
            "[[items]]\nkind = \"skill\"\nname = \"x\"\npath = \"skills/x\"\n",
        )
        .unwrap();
        let err = ItemToml::load(&dir).expect_err("an [[items]] table must be rejected");
        assert!(
            err.to_string().contains("items"),
            "the error must name the offending key: {err}"
        );
        assert!(
            err.to_string().contains(&manifest.display().to_string()),
            "the error must name the manifest it failed to read: {err}"
        );

        // Absent is absent, not an error.
        std::fs::remove_file(dir.join("mind.toml")).unwrap();
        assert!(ItemToml::load(&dir).expect("absent is ok").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_event_is_install() {
        // spec: HOOK-51
        let toml = r#"
            [[hooks]]
            run = "setup.sh"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].event, HookEvent::Install);
    }

    #[test]
    fn explicit_uninstall_event_resolves() {
        // spec: HOOK-51
        let toml = r#"
            [[hooks]]
            run = "teardown.sh"
            event = "uninstall"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].event, HookEvent::Uninstall);
    }

    #[test]
    fn unknown_event_returns_mind_toml_error() {
        // spec: HOOK-51
        let toml = r#"
            [[hooks]]
            run = "do-something.sh"
            event = "build"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let toml_path = Path::new("/repo/mind.toml");
        let result = parsed.resolved_hooks(toml_path);
        assert!(result.is_err(), "unknown event must error");
        let err = result.unwrap_err();
        // Must be a MindToml variant.
        match err {
            MindError::MindToml { path, msg } => {
                assert_eq!(path, toml_path);
                assert!(
                    msg.contains("build"),
                    "error message should mention the bad value: {msg}"
                );
                assert!(
                    msg.contains("install") && msg.contains("uninstall"),
                    "error message should mention valid values: {msg}"
                );
            }
            other => panic!("expected MindError::MindToml, got: {other:?}"),
        }
    }

    #[test]
    fn empty_run_in_hooks_is_dropped() {
        // spec: HOOK-51 (HOOK-3 generalized): blank run entries are silently skipped
        let toml = r#"
            [[hooks]]
            run = ""

            [[hooks]]
            run = "   "

            [[hooks]]
            run = "real-command"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "real-command");
    }

    #[test]
    fn whitespace_legacy_install_contributes_nothing() {
        // spec: HOOK-50 — whitespace-only install is ignored
        let toml = r#"
            [source]
            install = "   "

            [[hooks]]
            run = "npm ci"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        // Whitespace install contributes nothing; only the declared hook remains.
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "npm ci");
    }

    #[test]
    fn resolved_hook_label_returns_name_when_set() {
        // spec: HOOK-51
        let hook = ResolvedHook {
            run: "make build".into(),
            name: Some("Build step".into()),
            optional: false,
            event: HookEvent::Install,
        };
        assert_eq!(hook.label(), "Build step");
    }

    #[test]
    fn resolved_hook_label_falls_back_to_command() {
        // spec: HOOK-51
        let hook = ResolvedHook {
            run: "make build".into(),
            name: None,
            optional: false,
            event: HookEvent::Install,
        };
        assert_eq!(hook.label(), "make build");
    }

    #[test]
    fn label_treats_empty_and_whitespace_name_as_absent() {
        // spec: HOOK-51
        let make_hook = |name: Option<&str>| ResolvedHook {
            run: "make build".into(),
            name: name.map(str::to_owned),
            optional: false,
            event: HookEvent::Install,
        };

        // Non-empty name is used as-is.
        assert_eq!(make_hook(Some("Build")).label(), "Build");
        // Empty string falls back to run.
        assert_eq!(make_hook(Some("")).label(), "make build");
        // Whitespace-only falls back to run.
        assert_eq!(make_hook(Some("   ")).label(), "make build");
        // None falls back to run.
        assert_eq!(make_hook(None).label(), "make build");
    }

    #[test]
    fn hooks_whitespace_run_is_trimmed() {
        // spec: HOOK-51 — leading/trailing whitespace in run is trimmed
        let toml = r#"
            [[hooks]]
            run = "  npm install  "
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let dummy = Path::new("mind.toml");
        let resolved = parsed.resolved_hooks(dummy).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "npm install");
    }

    // ----- HOOK-86: [[items.hooks]] item-level array -----

    /// Parse a `mind.toml` and return its first item's resolved hooks.
    fn first_item_hooks(toml: &str) -> Result<Vec<ResolvedHook>> {
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let item = &parsed.items[0];
        item.resolved_item_hooks(Path::new("/repo/mind.toml"))
    }

    #[test]
    fn item_hooks_array_parses_and_preserves_order() {
        // spec: HOOK-86
        let toml = r#"
            [[items]]
            kind = "tool"
            name = "helper"
            path = "tools/helper"

            [[items.hooks]]
            run = "first"
            name = "Step one"

            [[items.hooks]]
            run = "second"
            optional = true
            event = "install"

            [[items.hooks]]
            run = "teardown"
            event = "uninstall"
        "#;
        let resolved = first_item_hooks(toml).expect("resolve");
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].run, "first");
        assert_eq!(resolved[0].name.as_deref(), Some("Step one"));
        assert!(!resolved[0].optional);
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert_eq!(resolved[1].run, "second");
        assert!(resolved[1].optional);
        assert_eq!(resolved[1].event, HookEvent::Install);
        assert_eq!(resolved[2].run, "teardown");
        assert_eq!(resolved[2].event, HookEvent::Uninstall);
    }

    #[test]
    fn item_scalar_install_uninstall_fold_in_ahead_of_array() {
        // spec: HOOK-86 — scalar install/uninstall are the one-required-hook
        // shorthand, folded in ahead of any [[items.hooks]] entries.
        let toml = r#"
            [[items]]
            kind = "tool"
            name = "helper"
            path = "tools/helper"
            install = "scalar-install"
            uninstall = "scalar-uninstall"

            [[items.hooks]]
            run = "array-install"
            event = "install"

            [[items.hooks]]
            run = "array-uninstall"
            event = "uninstall"
        "#;
        let resolved = first_item_hooks(toml).expect("resolve");
        assert_eq!(resolved.len(), 4);
        // Scalar install first, then scalar uninstall, then the array entries.
        assert_eq!(resolved[0].run, "scalar-install");
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert!(!resolved[0].optional);
        assert_eq!(resolved[0].name, None);
        assert_eq!(resolved[1].run, "scalar-uninstall");
        assert_eq!(resolved[1].event, HookEvent::Uninstall);
        assert!(!resolved[1].optional);
        assert_eq!(resolved[2].run, "array-install");
        assert_eq!(resolved[3].run, "array-uninstall");
    }

    #[test]
    fn item_scalar_only_yields_one_required_hook_each() {
        // spec: HOOK-86 — with no array, the scalars alone resolve to one
        // required hook of each event.
        let toml = r#"
            [[items]]
            kind = "rule"
            name = "style"
            path = "guidelines/style.md"
            install = "set-up"
            uninstall = "tear-down"
        "#;
        let resolved = first_item_hooks(toml).expect("resolve");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].run, "set-up");
        assert_eq!(resolved[0].event, HookEvent::Install);
        assert_eq!(resolved[1].run, "tear-down");
        assert_eq!(resolved[1].event, HookEvent::Uninstall);
    }

    #[test]
    fn item_hooks_unknown_event_is_mind_toml_error() {
        // spec: HOOK-86 — same validation as a source [[hooks]] (HOOK-51).
        let toml = r#"
            [[items]]
            kind = "tool"
            name = "helper"
            path = "tools/helper"

            [[items.hooks]]
            run = "do-something"
            event = "build"
        "#;
        let err = first_item_hooks(toml).unwrap_err();
        match err {
            MindError::MindToml { path, msg } => {
                assert_eq!(path, Path::new("/repo/mind.toml"));
                assert!(msg.contains("build"), "names the bad value: {msg}");
                assert!(
                    msg.contains("install") && msg.contains("uninstall"),
                    "names the legal set: {msg}"
                );
                // The message localizes which [[items]] declaration is at fault,
                // not just the file, so a multi-item mind.toml is diagnosable.
                assert!(msg.contains("helper"), "names the offending item: {msg}");
            }
            other => panic!("expected MindError::MindToml, got: {other:?}"),
        }
    }

    #[test]
    fn item_hooks_empty_run_is_dropped() {
        // spec: HOOK-86 — empty/whitespace run entries are silently dropped
        // (HOOK-3), as for a source hook.
        let toml = r#"
            [[items]]
            kind = "tool"
            name = "helper"
            path = "tools/helper"

            [[items.hooks]]
            run = ""

            [[items.hooks]]
            run = "   "

            [[items.hooks]]
            run = "real-command"
        "#;
        let resolved = first_item_hooks(toml).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "real-command");
    }

    #[test]
    fn item_scalar_whitespace_contributes_nothing() {
        // spec: HOOK-86 — a whitespace-only scalar install folds in nothing.
        let toml = r#"
            [[items]]
            kind = "tool"
            name = "helper"
            path = "tools/helper"
            install = "   "

            [[items.hooks]]
            run = "array-install"
        "#;
        let resolved = first_item_hooks(toml).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "array-install");
    }

    // ----- DSC-59: NestedSource curator-supplied fields -----

    #[test]
    fn nested_source_parses_all_curator_fields() {
        // Parses follow-branch, roots, and [[discover.sources.hooks]] in a
        // full MindToml round-trip using the canonical `namespace` key (DSC-78).
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            namespace = "or"
            install = true
            follow-branch = "main"
            roots = ["packages", "tools"]

            [[discover.sources.hooks]]
            run = "make setup"
            name = "Setup"
            optional = true
            event = "install"

            [[discover.sources.hooks]]
            run = "make teardown"
            event = "uninstall"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];

        assert_eq!(ns.source, "github:owner/repo");
        assert_eq!(ns.namespace.as_deref(), Some("or"));
        assert_eq!(ns.effective_alias(), Some("or".to_string()));
        assert!(ns.install);
        assert_eq!(ns.follow_branch.as_deref(), Some("main"));
        assert_eq!(
            ns.roots.as_deref(),
            Some(&["packages".to_owned(), "tools".to_owned()][..])
        );
        assert_eq!(ns.hooks.len(), 2);

        let h0 = &ns.hooks[0];
        assert_eq!(h0.run, "make setup");
        assert_eq!(h0.name.as_deref(), Some("Setup"));
        assert!(h0.optional);
        assert_eq!(h0.event.as_deref(), Some("install"));

        let h1 = &ns.hooks[1];
        assert_eq!(h1.run, "make teardown");
        assert_eq!(h1.event.as_deref(), Some("uninstall"));
    }

    #[test]
    fn nested_source_deny_unknown_fields_still_rejects_typo() {
        // deny_unknown_fields must still reject a misspelled key.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            follow_branch = "main"
        "#;
        let result: std::result::Result<MindToml, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "underscore variant of follow-branch must be rejected"
        );
    }

    #[test]
    fn nested_source_unknown_top_level_key_rejected() {
        // A completely unknown key at the nested-source level is an error.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            typo-key = "value"
        "#;
        let result: std::result::Result<MindToml, _> = toml::from_str(toml);
        assert!(result.is_err(), "unknown keys must be rejected");
    }

    // ----- DSC-78: namespace key and effective_alias() -----

    #[test]
    fn namespace_key_parses_as_canonical_form() {
        // spec: DSC-78
        // The canonical `namespace` key is parsed into NestedSource::namespace;
        // the legacy `alias` field stays None, and effective_alias() returns the value.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            namespace = "pfx"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert_eq!(ns.namespace.as_deref(), Some("pfx"));
        assert!(ns.alias.is_none());
        assert_eq!(ns.effective_alias(), Some("pfx".to_string()));
    }

    #[test]
    fn as_key_still_accepted_as_legacy_alias() {
        // spec: DSC-78
        // The legacy `as` key still deserializes into NestedSource::alias for
        // backwards compatibility; namespace stays None.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            as = "pfx"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert_eq!(ns.alias.as_deref(), Some("pfx"));
        assert!(ns.namespace.is_none());
        assert_eq!(ns.effective_alias(), Some("pfx".to_string()));
    }

    #[test]
    fn namespace_takes_precedence_over_as_when_both_present() {
        // spec: DSC-78
        // When both `namespace` and `as` are present, effective_alias() returns
        // the `namespace` value. Both keys map to distinct struct fields, so
        // TOML accepts the snippet and deny_unknown_fields is satisfied.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            namespace = "ns-wins"
            as = "as-loses"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert_eq!(ns.effective_alias(), Some("ns-wins".to_string()));
    }

    #[test]
    fn effective_alias_returns_none_when_neither_set() {
        // spec: DSC-78
        // When neither namespace nor alias is set, effective_alias() is None.
        let toml = "[[discover.sources]]\nsource = \"github:owner/repo\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert!(ns.namespace.is_none());
        assert!(ns.alias.is_none());
        assert_eq!(ns.effective_alias(), None);
    }

    #[test]
    fn nested_source_pin_directive_returns_follow_branch_pin() {
        // spec: DSC-59 — pin_directive returns Pin::FollowBranch for follow-branch entries.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: Some("develop".into()),
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        assert_eq!(
            ns.pin_directive(Path::new("mind.toml")).expect("no error"),
            Some(Pin::FollowBranch("develop".into()))
        );
    }

    #[test]
    fn nested_source_pin_directive_none_when_unset() {
        // spec: DSC-59 — no pin fields => None.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        assert!(
            ns.pin_directive(Path::new("mind.toml"))
                .expect("no error")
                .is_none()
        );
    }

    #[test]
    fn nested_source_resolved_hooks_order_preserved() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = "first"

            [[discover.sources.hooks]]
            run = "second"

            [[discover.sources.hooks]]
            run = "third"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].run, "first");
        assert_eq!(resolved[1].run, "second");
        assert_eq!(resolved[2].run, "third");
    }

    #[test]
    fn nested_source_resolved_hooks_default_event_is_install() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = "setup.sh"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].event, HookEvent::Install);
    }

    #[test]
    fn nested_source_resolved_hooks_explicit_uninstall() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = "teardown.sh"
            event = "uninstall"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].event, HookEvent::Uninstall);
    }

    #[test]
    fn nested_source_resolved_hooks_unknown_event_is_mind_toml_error() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = "do-something"
            event = "build"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let toml_path = Path::new("/repo/mind.toml");
        let err = ns.resolved_hooks(toml_path).unwrap_err();
        match err {
            MindError::MindToml { path, msg } => {
                assert_eq!(path, toml_path);
                assert!(msg.contains("build"), "names the bad value: {msg}");
                assert!(
                    msg.contains("install") && msg.contains("uninstall"),
                    "names the legal set: {msg}"
                );
            }
            other => panic!("expected MindError::MindToml, got: {other:?}"),
        }
    }

    #[test]
    fn nested_source_resolved_hooks_blank_run_dropped() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = ""

            [[discover.sources.hooks]]
            run = "   "

            [[discover.sources.hooks]]
            run = "real-command"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "real-command");
    }

    #[test]
    fn nested_source_resolved_hooks_run_trimmed() {
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"

            [[discover.sources.hooks]]
            run = "  npm install  "
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "npm install");
    }

    #[test]
    fn nested_source_no_legacy_install_fold_in() {
        // NestedSource::resolved_hooks has NO [source].install fold-in; only
        // the [[discover.sources.hooks]] array is considered.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![Hook {
                run: "array-only".into(),
                name: None,
                optional: false,
                event: None,
            }],
            flat_skills: false,
            on_auth_failure: None,
        };
        let resolved = ns.resolved_hooks(Path::new("mind.toml")).expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].run, "array-only");
    }

    #[test]
    fn nested_source_follow_branch_pin_directive_is_always_follow_branch_variant() {
        // spec: DSC-59
        // When follow_branch is set and pin_tag/pin_ref are not, pin_directive
        // always returns a FollowBranch variant carrying the exact branch string.
        for branch in ["main", "develop", "v1", "release/2.0", "abc123"] {
            let ns = NestedSource {
                source: "x".into(),
                namespace: None,
                kind: None,
                alias: None,
                install: false,
                install_items: None,
                profiles: None,
                default_profile: None,
                follow_branch: Some(branch.into()),
                pin_tag: None,
                pin_ref: None,
                pin_version: None,
                roots: None,
                add_roots: None,
                hooks: vec![],
                flat_skills: false,
                on_auth_failure: None,
            };
            match ns.pin_directive(Path::new("mind.toml")).expect("no error") {
                Some(Pin::FollowBranch(b)) => assert_eq!(b, branch),
                other => panic!("expected Pin::FollowBranch({branch:?}); got {other:?}"),
            }
        }
    }

    #[test]
    fn multiple_nested_sources_keep_independent_hook_arrays() {
        // spec: DSC-59
        // Each `[[discover.sources]]` entry owns its own
        // `[[discover.sources.hooks]]` array; one entry's hooks must not leak into
        // another's. Two entries with distinct hooks must resolve to disjoint hook
        // lists, so the per-entry CuratedConfig the caller builds stays isolated.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/first"

            [[discover.sources.hooks]]
            run = "first-hook"

            [[discover.sources]]
            source = "github:owner/second"

            [[discover.sources.hooks]]
            run = "second-hook"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let sources = &parsed.discover.as_ref().unwrap().sources;
        assert_eq!(sources.len(), 2);

        let dummy = Path::new("mind.toml");
        let first = sources[0].resolved_hooks(dummy).expect("resolve");
        let second = sources[1].resolved_hooks(dummy).expect("resolve");

        assert_eq!(first.len(), 1, "first entry sees only its own hook");
        assert_eq!(first[0].run, "first-hook");
        assert_eq!(second.len(), 1, "second entry sees only its own hook");
        assert_eq!(second[0].run, "second-hook");
        // No cross-contamination: neither entry carries the other's hook command.
        assert!(
            first.iter().all(|h| h.run != "second-hook"),
            "second entry's hook leaked into the first"
        );
        assert!(
            second.iter().all(|h| h.run != "first-hook"),
            "first entry's hook leaked into the second"
        );
    }

    #[test]
    fn nested_source_explicit_empty_roots_is_some_empty_not_none() {
        // spec: DSC-59
        // A curator `roots = []` (explicit empty list) parses to Some(empty),
        // distinct from an unset `roots` (None). The catalog scan mirrors source
        // roots: Some(empty) scans zero roots (discovers nothing) while None falls
        // back to the repo root. Preserving the distinction at the parse layer is
        // what lets the apply path honor "scan nothing".
        let explicit = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            roots = []
        "#;
        let parsed: MindToml = toml::from_str(explicit).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert_eq!(
            ns.roots.as_deref(),
            Some(&[][..]),
            "explicit roots = [] must be Some(empty), not None"
        );

        let unset = r#"
            [[discover.sources]]
            source = "github:owner/repo"
        "#;
        let parsed: MindToml = toml::from_str(unset).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert!(
            ns.roots.is_none(),
            "unset roots must be None, distinct from the empty list"
        );
    }

    // ----- DSC-62 / DSC-63 / DSC-64: install_items subset directive -----

    #[test]
    fn install_items_absent_parses_to_none() {
        // spec: DSC-62 — when install-items is absent, the field is None and
        // the install bool governs as before (DSC-58).
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            install = true
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert!(
            ns.install_items.is_none(),
            "absent install-items must be None"
        );
        assert!(ns.install, "install = true must still parse");
    }

    #[test]
    fn install_items_empty_list_parses_to_some_empty() {
        // spec: DSC-62 — install-items = [] is distinct from absent and
        // equivalent to install = false (offer nothing).
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            install-items = []
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert_eq!(
            ns.install_items.as_deref(),
            Some(&[][..]),
            "install-items = [] must be Some(empty)"
        );
    }

    #[test]
    fn install_items_non_empty_list_parses_correctly() {
        // spec: DSC-62 / DSC-63 — install-items carries bare kind:name refs.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            install-items = ["skill:review", "agent:dev"]
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let items = ns.install_items.as_deref().expect("must be Some");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], "skill:review");
        assert_eq!(items[1], "agent:dev");
    }

    #[test]
    fn install_items_three_way_distinction() {
        // spec: DSC-62 — None (absent) vs Some([]) (empty) vs Some([..]) (non-empty)
        // are three distinct states with different install semantics.
        let absent: MindToml = toml::from_str("[[discover.sources]]\nsource = \"x\"\n").unwrap();
        let empty: MindToml =
            toml::from_str("[[discover.sources]]\nsource = \"x\"\ninstall-items = []\n").unwrap();
        let nonempty: MindToml = toml::from_str(
            "[[discover.sources]]\nsource = \"x\"\ninstall-items = [\"skill:foo\"]\n",
        )
        .unwrap();

        let ns_absent = &absent.discover.as_ref().unwrap().sources[0];
        let ns_empty = &empty.discover.as_ref().unwrap().sources[0];
        let ns_nonempty = &nonempty.discover.as_ref().unwrap().sources[0];

        assert!(ns_absent.install_items.is_none(), "absent => None");
        assert_eq!(
            ns_empty.install_items.as_deref(),
            Some(&[][..]),
            "empty => Some([])"
        );
        assert_eq!(
            ns_nonempty.install_items.as_deref().unwrap().len(),
            1,
            "non-empty => Some([..])"
        );
    }

    #[test]
    fn install_true_with_nonempty_install_items_is_mind_toml_error() {
        // spec: DSC-64 — install = true and a non-empty install-items on the same
        // entry is a MindToml error (mutually exclusive).
        let ns = NestedSource {
            source: "github:owner/repo".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: true,
            install_items: Some(vec!["skill:review".into()]),
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let toml_path = Path::new("/super/mind.toml");
        let err = ns.validate(toml_path).unwrap_err();
        match err {
            MindError::MindToml { path, msg } => {
                assert_eq!(path, toml_path);
                assert!(
                    msg.contains("mutually exclusive"),
                    "error must say mutually exclusive: {msg}"
                );
                assert!(
                    msg.contains("github:owner/repo"),
                    "error must name the source: {msg}"
                );
            }
            other => panic!("expected MindError::MindToml, got: {other:?}"),
        }
    }

    #[test]
    fn install_true_with_empty_install_items_is_ok() {
        // spec: DSC-64 — install = true and install-items = [] is NOT an error:
        // the empty list is equivalent to install = false and overrides the boolean,
        // so there is no contradiction. Only a non-empty list conflicts.
        let ns = NestedSource {
            source: "github:owner/repo".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: true,
            install_items: Some(vec![]), // empty is fine
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        assert!(
            ns.validate(Path::new("mind.toml")).is_ok(),
            "install = true + install-items = [] must not error (both say 'install nothing')"
        );
    }

    #[test]
    fn install_false_with_nonempty_install_items_is_ok() {
        // spec: DSC-62 — the normal subset form: install-items alone (install
        // left false/unset). Must not error from validate().
        let ns = NestedSource {
            source: "github:owner/repo".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: Some(vec!["skill:review".into(), "agent:dev".into()]),
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        assert!(
            ns.validate(Path::new("mind.toml")).is_ok(),
            "install-items alone (install = false) must not error"
        );
    }

    // ----- DSC-59 / DSC-65: NestedSource pin_directive (all three forms) -----

    #[test]
    fn nested_pin_directive_none_when_no_pin_set() {
        // spec: DSC-59
        let toml = "[[discover.sources]]\nsource = \"github:owner/repo\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let pin = ns
            .pin_directive(Path::new("mind.toml"))
            .expect("no conflict");
        assert!(pin.is_none(), "no pin set => None");
    }

    #[test]
    fn nested_pin_directive_follow_branch() {
        // spec: DSC-59
        let toml =
            "[[discover.sources]]\nsource = \"github:owner/repo\"\nfollow-branch = \"main\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let pin = ns
            .pin_directive(Path::new("mind.toml"))
            .expect("no conflict");
        assert_eq!(pin, Some(Pin::FollowBranch("main".into())));
    }

    #[test]
    fn nested_pin_directive_pin_tag_parses_and_resolves() {
        // spec: DSC-59 — pin-tag is now accepted on a nested entry.
        let toml = "[[discover.sources]]\nsource = \"github:owner/repo\"\npin-tag = \"v2.0\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let pin = ns
            .pin_directive(Path::new("mind.toml"))
            .expect("no conflict");
        assert_eq!(pin, Some(Pin::Tag("v2.0".into())));
    }

    #[test]
    fn nested_pin_directive_pin_ref_parses_and_resolves() {
        // spec: DSC-59 DSC-65 — pin-ref is accepted on a nested entry and is
        // the form emitted by `mind dump` for exact-revision reproduction.
        let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let toml =
            format!("[[discover.sources]]\nsource = \"github:owner/repo\"\npin-ref = \"{sha}\"\n");
        let parsed: MindToml = toml::from_str(&toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let pin = ns
            .pin_directive(Path::new("mind.toml"))
            .expect("no conflict");
        assert_eq!(pin, Some(Pin::Ref(sha.into())));
    }

    #[test]
    fn nested_pin_directive_conflict_follow_branch_and_pin_tag_is_error() {
        // spec: DSC-59 — more than one pin directive on a nested entry is a
        // MindToml error (the same one-of rule as DSC-41 on [source]).
        let toml = "[[discover.sources]]\nsource = \"github:owner/repo\"\nfollow-branch = \"main\"\npin-tag = \"v1\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let result = ns.pin_directive(Path::new("/super/mind.toml"));
        assert!(result.is_err(), "conflicting pin directives must error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("conflicting pin"),
            "error must mention 'conflicting pin': {err_msg}"
        );
    }

    #[test]
    fn nested_pin_directive_conflict_all_three_is_error() {
        // spec: DSC-59 — all three directives at once is also a conflict error.
        let toml = "[[discover.sources]]\nsource = \"github:owner/repo\"\nfollow-branch = \"main\"\npin-tag = \"v1\"\npin-ref = \"abc123\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let result = ns.pin_directive(Path::new("/super/mind.toml"));
        assert!(result.is_err(), "three pin directives must error");
    }

    // ----- DSC-66: pin value validation in SourceMeta -----

    #[test]
    fn source_meta_pin_rejects_leading_dash_in_follow_branch() {
        // spec: DSC-66 — a [source] follow-branch value beginning with `-` is
        // rejected at parse time to prevent git argument injection.
        let meta = SourceMeta {
            follow_branch: Some("--upload-pack=touch /tmp/pwned".into()),
            ..Default::default()
        };
        let err = meta.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn source_meta_pin_rejects_leading_dash_in_pin_tag() {
        // spec: DSC-66
        let meta = SourceMeta {
            pin_tag: Some("-malicious".into()),
            ..Default::default()
        };
        let err = meta.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn source_meta_pin_rejects_leading_dash_in_pin_ref() {
        // spec: DSC-66
        let meta = SourceMeta {
            pin_ref: Some("--no-tags".into()),
            ..Default::default()
        };
        let err = meta.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn source_meta_pin_rejects_empty_follow_branch() {
        // spec: DSC-66 — empty string is invalid.
        let meta = SourceMeta {
            follow_branch: Some("".into()),
            ..Default::default()
        };
        let err = meta.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn source_meta_pin_accepts_valid_values() {
        // spec: DSC-66 — well-formed values pass validation.
        for (field, pin) in [
            ("follow-branch", "main"),
            ("pin-tag", "v2.0"),
            ("pin-ref", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        ] {
            let meta = match field {
                "follow-branch" => SourceMeta {
                    follow_branch: Some(pin.into()),
                    ..Default::default()
                },
                "pin-tag" => SourceMeta {
                    pin_tag: Some(pin.into()),
                    ..Default::default()
                },
                _ => SourceMeta {
                    pin_ref: Some(pin.into()),
                    ..Default::default()
                },
            };
            assert!(
                meta.pin_directive(Path::new("mind.toml")).is_ok(),
                "expected ok for {field}={pin:?}"
            );
        }
    }

    // ----- DSC-66: pin value validation in NestedSource -----

    #[test]
    fn nested_source_pin_rejects_leading_dash_in_follow_branch() {
        // spec: DSC-66 — a nested-source follow-branch beginning with `-` is
        // rejected at parse time to prevent git argument injection.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: Some("--upload-pack=touch /tmp/pwned".into()),
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let err = ns.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn nested_source_pin_rejects_leading_dash_in_pin_tag() {
        // spec: DSC-66
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: Some("-evil".into()),
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let err = ns.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn nested_source_pin_rejects_leading_dash_in_pin_ref() {
        // spec: DSC-66
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("--depth=1".into()),
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let err = ns.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef, got: {err}"
        );
    }

    #[test]
    fn nested_source_pin_rejects_whitespace_in_ref() {
        // spec: DSC-66 — whitespace in a pin value is rejected.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: Some("main branch".into()),
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let err = ns.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef for whitespace in branch name, got: {err}"
        );
    }

    #[test]
    fn nested_source_pin_rejects_dotdot_in_ref() {
        // spec: DSC-66 — '..' in a pin value (git range syntax) is rejected.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("main..HEAD".into()),
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        let err = ns.pin_directive(Path::new("mind.toml")).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRef { .. }),
            "expected InvalidRef for '..' in ref, got: {err}"
        );
    }

    #[test]
    fn nested_source_pin_accepts_valid_values() {
        // spec: DSC-66 — well-formed values pass.
        let ns = NestedSource {
            source: "x".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("cafebabecafebabecafebabecafebabecafebabe".into()),
            pin_version: None,
            roots: None,
            add_roots: None,
            hooks: vec![],
            flat_skills: false,
            on_auth_failure: None,
        };
        assert!(
            ns.pin_directive(Path::new("mind.toml")).is_ok(),
            "valid SHA must pass pin validation"
        );
    }

    // ----- DSC-66: end-to-end rejection from an on-disk untrusted mind.toml -----

    /// Write `text` as `mind.toml` in a fresh temp dir and return the dir.
    fn write_mind_toml(text: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-dsc66-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), text).unwrap();
        dir
    }

    #[test]
    fn untrusted_source_mind_toml_pin_is_rejected_before_any_git_call() {
        // spec: DSC-66 - end-to-end: a malicious `[source]` pin shipped in a
        // real on-disk mind.toml is rejected at parse time (load + pin_directive)
        // with InvalidRef, BEFORE any git subprocess could run. This exercises
        // the actual file-load entry point (MindToml::load), not a hand-built
        // struct, so it proves the injection barrier sits ahead of the git layer.
        for (field, payload) in [
            ("follow-branch", "--upload-pack=touch /tmp/pwned"),
            ("pin-tag", "--upload-pack=touch /tmp/pwned"),
            ("pin-ref", "--upload-pack=touch /tmp/pwned"),
        ] {
            let dir = write_mind_toml(&format!("[source]\n{field} = \"{payload}\"\n"));
            let parsed = MindToml::load(&dir)
                .expect("load ok")
                .expect("mind.toml present");
            let err = parsed
                .source
                .pin_directive(&dir.join("mind.toml"))
                .expect_err("malicious pin must be rejected");
            assert!(
                matches!(err, MindError::InvalidRef { .. }),
                "expected InvalidRef for {field}={payload:?}, got: {err}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn untrusted_nested_source_mind_toml_pin_is_rejected_before_any_git_call() {
        // spec: DSC-66 - end-to-end for a curated super-source: a malicious pin on
        // a `[[discover.sources]]` entry of a real on-disk mind.toml is rejected at
        // parse time with InvalidRef before any git subprocess. A super-source's
        // mind.toml is attacker-controlled content, so this is the headline threat.
        for (field, payload) in [
            ("follow-branch", "--upload-pack=touch /tmp/pwned"),
            ("pin-tag", "-x"),
            ("pin-ref", "main..HEAD"),
        ] {
            let dir = write_mind_toml(&format!(
                "[[discover.sources]]\nsource = \"github:owner/repo\"\n{field} = \"{payload}\"\n"
            ));
            let parsed = MindToml::load(&dir)
                .expect("load ok")
                .expect("mind.toml present");
            let ns = &parsed.discover.as_ref().unwrap().sources[0];
            let err = ns
                .pin_directive(&dir.join("mind.toml"))
                .expect_err("malicious nested pin must be rejected");
            assert!(
                matches!(err, MindError::InvalidRef { .. }),
                "expected InvalidRef for nested {field}={payload:?}, got: {err}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn untrusted_mind_toml_with_legitimate_pin_loads_and_resolves() {
        // spec: DSC-66 - the validation barrier must not over-reject: a real
        // on-disk mind.toml carrying a legitimate pin (branch with a slash, a
        // dotted tag, a 40-hex sha) loads and resolves to the expected Pin.
        let cases = [
            (
                "follow-branch = \"release/2.0\"",
                Pin::FollowBranch("release/2.0".into()),
            ),
            ("pin-tag = \"v1.2.3\"", Pin::Tag("v1.2.3".into())),
            (
                "pin-ref = \"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\"",
                Pin::Ref("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into()),
            ),
        ];
        for (line, expect) in cases {
            let dir = write_mind_toml(&format!("[source]\n{line}\n"));
            let parsed = MindToml::load(&dir).unwrap().unwrap();
            let pin = parsed
                .source
                .pin_directive(&dir.join("mind.toml"))
                .expect("legitimate pin must resolve");
            assert_eq!(pin, Some(expect), "for line {line:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn nested_pin_directive_pin_ref_parses_back_from_dump_output() {
        // spec: DSC-65 DUMP-1 — `mind dump` emits `pin-ref = <sha>`; when the
        // output is melded the nested entry's pin_directive returns Pin::Ref.
        // Verify the round-trip: serialise a DumpEntry-style string manually
        // and parse it back as a MindToml nested source.
        let sha = "cafebabecafebabecafebabecafebabecafebabe";
        // Simulate what dump emits (after our dump.rs change):
        let toml_text = format!(
            "[source]\ndescription = \"Generated by mind dump.\"\n\
             [[discover.sources]]\nsource = \"/path/to/repo\"\npin-ref = \"{sha}\"\ninstall = false\n"
        );
        let parsed: MindToml = toml::from_str(&toml_text).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let pin = ns
            .pin_directive(Path::new("mind.toml"))
            .expect("no conflict");
        assert_eq!(
            pin,
            Some(Pin::Ref(sha.into())),
            "pin-ref from dump output must round-trip as Pin::Ref"
        );
    }

    // ----- DSC-68: on-auth-failure field on NestedSource -----

    #[test]
    fn on_auth_failure_skip_parses() {
        // spec: DSC-68
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/private-repo"
            on-auth-failure = { action = "skip" }
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let cfg = ns
            .on_auth_failure
            .as_ref()
            .expect("on-auth-failure must be Some");
        assert_eq!(cfg.action, AuthFailureAction::Skip);
        assert!(cfg.message.is_none());
    }

    #[test]
    fn on_auth_failure_error_with_message_parses() {
        // spec: DSC-68
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/private-repo"
            on-auth-failure = { action = "error", message = "Configure credentials: https://example.com/auth" }
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let cfg = ns
            .on_auth_failure
            .as_ref()
            .expect("on-auth-failure must be Some");
        assert_eq!(cfg.action, AuthFailureAction::Error);
        assert_eq!(
            cfg.message.as_deref(),
            Some("Configure credentials: https://example.com/auth")
        );
    }

    #[test]
    fn on_auth_failure_absent_parses_to_none() {
        // spec: DSC-68 -- without on-auth-failure, the field is absent
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        assert!(
            ns.on_auth_failure.is_none(),
            "absent on-auth-failure must be None"
        );
    }

    #[test]
    fn on_auth_failure_unknown_action_is_mind_toml_error() {
        // spec: DSC-68 -- serde rejects an unknown action variant at parse time;
        // validate() is no longer the enforcement point now that action is a typed enum.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            on-auth-failure = { action = "warn" }
        "#;
        let result: std::result::Result<MindToml, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "unknown on-auth-failure action must fail at parse time"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("warn") || err_msg.contains("error") || err_msg.contains("expected"),
            "error must describe the invalid variant: {err_msg}"
        );
    }

    #[test]
    fn on_auth_failure_validate_with_skip_returns_ok() {
        // spec: DSC-68 -- a NestedSource with a well-formed on-auth-failure passes validate()
        let ns = NestedSource {
            source: "github:owner/repo".into(),
            namespace: None,
            kind: None,
            alias: None,
            install: false,
            install_items: None,
            profiles: None,
            default_profile: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            pin_version: None,
            roots: None,
            add_roots: None,
            flat_skills: false,
            hooks: vec![],
            on_auth_failure: Some(OnAuthFailure {
                action: AuthFailureAction::Skip,
                message: None,
            }),
        };
        assert!(
            ns.validate(Path::new("mind.toml")).is_ok(),
            "a valid on-auth-failure must not error from validate()"
        );
    }

    #[test]
    fn on_auth_failure_unknown_field_in_inline_table_is_parse_error() {
        // spec: DSC-68 -- unknown fields in on-auth-failure inline table are rejected
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            on-auth-failure = { action = "skip", typo-key = "x" }
        "#;
        let result: std::result::Result<MindToml, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "unknown field in on-auth-failure must be rejected"
        );
    }

    #[test]
    fn on_auth_failure_skip_with_message_parses() {
        // spec: DSC-68 -- the example from the spec
        let toml = r#"
            [[discover.sources]]
            source = "owner/private-repo"
            on-auth-failure = { action = "skip", message = "Configure credentials: https://example.com/auth" }
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        let ns = &parsed.discover.as_ref().unwrap().sources[0];
        let cfg = ns
            .on_auth_failure
            .as_ref()
            .expect("on-auth-failure must be Some");
        assert_eq!(cfg.action, AuthFailureAction::Skip);
        assert_eq!(
            cfg.message.as_deref(),
            Some("Configure credentials: https://example.com/auth")
        );
    }

    // ---- DSC-81: [discover] glob confinement at parse time -------------------

    use std::sync::atomic::{AtomicU32, Ordering};
    static MINDFILE_N: AtomicU32 = AtomicU32::new(0);

    fn make_tmp_root() -> std::path::PathBuf {
        let n = MINDFILE_N.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("mind-mindfile-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn discover_glob_absolute_pattern_is_rejected_at_load() {
        // spec: DSC-81 -- an absolute discover glob is rejected when the
        // mind.toml is loaded, before any filesystem glob expansion.
        let root = make_tmp_root();
        std::fs::write(
            root.join("mind.toml"),
            "[discover]\nagents = { include = [\"/etc/*.md\"] }\n",
        )
        .unwrap();
        let err = MindToml::load(&root).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("absolute"),
            "error must mention 'absolute': {msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_glob_parent_dir_pattern_is_rejected_at_load() {
        // spec: DSC-81 -- a `..` component in a discover glob is rejected at load.
        let root = make_tmp_root();
        std::fs::write(
            root.join("mind.toml"),
            "[discover]\nskills = { include = [\"../other-repo/skills/*/SKILL.md\"] }\n",
        )
        .unwrap();
        let err = MindToml::load(&root).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(".."), "error must mention '..': {msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_glob_exclude_absolute_pattern_is_rejected_at_load() {
        // spec: DSC-81 -- the confinement check also applies to exclude patterns.
        let root = make_tmp_root();
        std::fs::write(
            root.join("mind.toml"),
            "[discover]\nagents = { include = [\"agents/*.md\"], exclude = [\"/tmp/*.md\"] }\n",
        )
        .unwrap();
        let err = MindToml::load(&root).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("absolute"),
            "error must mention 'absolute': {msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- DSC-82: `namespace` as alias for `prefix` in [source] ---------------

    #[test]
    fn source_prefix_parses_via_namespace_alias() {
        // spec: DSC-82 -- [source].namespace is an accepted alias for [source].prefix
        // at the serde level, so repos that use the new canonical key still parse.
        let toml = "[source]\nnamespace = \"jk\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert_eq!(
            parsed.source.prefix.as_deref(),
            Some("jk"),
            "namespace alias must populate the prefix field: {parsed:?}"
        );
    }

    #[test]
    fn source_prefix_parses_via_legacy_prefix_key() {
        // spec: DSC-82 -- the existing `prefix` key still works for backwards compat.
        let toml = "[source]\nprefix = \"jk\"\n";
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert_eq!(
            parsed.source.prefix.as_deref(),
            Some("jk"),
            "legacy prefix key must still work: {parsed:?}"
        );
    }

    // ---- PROF: install profiles ---------------------------------------------

    /// A fresh temp dir holding `mind.toml` = `body`.
    fn prof_dir(body: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-prof-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), body).unwrap();
        dir
    }

    // spec: PROF-1
    #[test]
    fn profiles_table_parses_and_is_not_authoritative() {
        let dir = prof_dir(
            "[profiles]\ndefault = [\"basic\"]\nbasic = [\"skill:review\", \"rule:style\"]\n\
             full = [\"basic\", \"agent:*\"]\nall = [\"*\"]\n",
        );
        let mt = MindToml::load(&dir).unwrap().unwrap();
        assert!(
            !mt.is_authoritative(),
            "[profiles] alone must keep convention discovery"
        );
        assert!(!mt.declares_scan_layout());
        let p = mt.profiles.as_ref().expect("profiles parsed");
        assert_eq!(p["full"], vec!["basic", "agent:*"]);
        assert_eq!(p.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);

        // Alongside [[items]], authority comes from [[items]], not [profiles].
        let dir = prof_dir(
            "[profiles]\np = [\"rule:style\"]\n\
             [[items]]\nkind = \"rule\"\nname = \"style\"\npath = \"r.md\"\n",
        );
        assert!(MindToml::load(&dir).unwrap().unwrap().is_authoritative());
        let _ = std::fs::remove_dir_all(&dir);

        // No [profiles]: None.
        let dir = prof_dir("[source]\ndescription = \"x\"\n");
        assert!(MindToml::load(&dir).unwrap().unwrap().profiles.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: VER-1
    #[test]
    fn release_tags_parse_and_are_not_authoritative() {
        for (body, want) in [
            (
                "[source]\nrelease-tags = \"skills-v{version}\"\n",
                Some(Template::Literal {
                    prefix: "skills-v".into(),
                    suffix: String::new(),
                }),
            ),
            ("[source]\nrelease-tags = true\n", Some(Template::Default)),
            ("[source]\ndescription = \"x\"\n", None),
        ] {
            let dir = prof_dir(body);
            let mt = MindToml::load(&dir).unwrap().unwrap();
            assert!(!mt.is_authoritative(), "{body:?}");
            assert_eq!(
                mt.source.release_template(Path::new("mind.toml")).unwrap(),
                want,
                "{body:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // spec: VER-2 VER-46
    #[test]
    fn invalid_release_tags_or_yanked_fail_load_as_mindtoml() {
        for (body, needle) in [
            (
                "[source]\nrelease-tags = false\n",
                "must be true or a template",
            ),
            ("[source]\nrelease-tags = \"v\"\n", "exactly one {version}"),
            (
                "[source]\nrelease-tags = \"{version}-{version}\"\n",
                "exactly one {version}",
            ),
            (
                "[source]\nrelease-tags = \"{name}-{version}\"\n",
                "no token other than",
            ),
            (
                "[source]\nrelease-tags = \"-v{version}\"\n",
                "begin with '-'",
            ),
            ("[source]\nyanked = [\"1.2\"]\n", "not a semver version"),
        ] {
            let dir = prof_dir(body);
            match MindToml::load(&dir) {
                Err(MindError::MindToml { msg, .. }) => {
                    assert!(msg.contains(needle), "{body:?}: {msg}");
                }
                other => panic!("{body:?}: expected MindToml, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        // A non-string, non-bool value fails the schema itself.
        let dir = prof_dir("[source]\nrelease-tags = 3\n");
        assert!(MindToml::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        let dir = prof_dir("[source]\nyanked = [\"1.2.3\", \"2.0.0-rc.1\"]\n");
        let mt = MindToml::load(&dir).unwrap().unwrap();
        let yanked: Vec<String> = mt
            .source
            .yanked_versions(Path::new("mind.toml"))
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(yanked, ["1.2.3", "2.0.0-rc.1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: PROF-2 PROF-3 PROF-4
    #[test]
    fn invalid_profiles_table_fails_load_as_mindtoml() {
        for (body, needle) in [
            ("[profiles]\nBad = []\n", "profile name 'Bad'"),
            ("[profiles]\np = [\"widget:x\"]\n", "unknown kind 'widget'"),
            ("[profiles]\np = [\"nope\"]\n", "does not declare"),
            ("[profiles]\np = [\"p\"]\n", "cycle: p -> p"),
        ] {
            let dir = prof_dir(body);
            match MindToml::load(&dir) {
                Err(MindError::MindToml { msg, .. }) => {
                    assert!(msg.starts_with("[profiles]: "), "{msg}");
                    assert!(msg.contains(needle), "{body:?}: {msg}");
                }
                other => panic!("{body:?}: expected MindToml, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // spec: PROF-8
    #[test]
    fn profiles_in_a_scoped_item_manifest_are_rejected() {
        let dir = prof_dir("[profiles]\nbasic = [\"skill:x\"]\n");
        match ItemToml::load(&dir) {
            Err(MindError::MindToml { msg, .. }) => {
                assert!(msg.contains("[profiles]"), "{msg}");
                assert!(msg.contains("root mind.toml only"), "{msg}");
            }
            other => panic!("expected MindToml, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        // Hooks alone still load.
        let dir = prof_dir("[[hooks]]\nrun = \"true\"\n");
        assert!(ItemToml::load(&dir).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn nested(body: &str) -> NestedSource {
        let mt: MindToml = toml::from_str(body).expect("parse");
        mt.discover.unwrap().sources.into_iter().next().unwrap()
    }

    // spec: PROF-26
    #[test]
    fn nested_profiles_parse_and_validate() {
        let ns = nested(
            "[discover]\nsources = [{ source = \"o/r\", profiles = [\"full\"], \
             default-profile = false, install-items = [\"skill:x\"] }]\n",
        );
        assert_eq!(ns.profiles.as_deref(), Some(&["full".to_string()][..]));
        assert_eq!(ns.default_profile, Some(false));
        ns.validate(Path::new("mind.toml"))
            .expect("profiles with install-items is allowed (the union installs)");
        let sel = crate::profile::Selection::from_nested(&ns);
        assert!(!sel.include_default);
        assert_eq!(sel.named, vec!["full"]);

        let plain = nested("[discover]\nsources = [{ source = \"o/r\" }]\n");
        assert_eq!(plain.profiles, None);
        assert_eq!(plain.default_profile, None);
        assert!(crate::profile::Selection::from_nested(&plain).include_default);

        let both = nested(
            "[discover]\nsources = [{ source = \"o/r\", install = true, profiles = [\"full\"] }]\n",
        );
        match both.validate(Path::new("mind.toml")) {
            Err(MindError::MindToml { msg, .. }) => {
                assert!(msg.contains("mutually exclusive"), "{msg}");
                assert!(msg.contains("o/r"), "{msg}");
            }
            other => panic!("expected MindToml, got {other:?}"),
        }

        let bad = nested("[discover]\nsources = [{ source = \"o/r\", profiles = [\"Full\"] }]\n");
        match bad.validate(Path::new("mind.toml")) {
            Err(MindError::MindToml { msg, .. }) => assert!(msg.contains("'Full'"), "{msg}"),
            other => panic!("expected MindToml, got {other:?}"),
        }
    }

    // spec: PROF-26
    #[test]
    fn nested_profiles_edge_cases() {
        let p = Path::new("mind.toml");
        // An empty profiles list with install = true is not a conflict.
        nested("[discover]\nsources = [{ source = \"o/r\", install = true, profiles = [] }]\n")
            .validate(p)
            .expect("empty profiles does not conflict with install");
        // default-profile alone, and = true, keep default enabled.
        let t = nested("[discover]\nsources = [{ source = \"o/r\", default-profile = true }]\n");
        t.validate(p).unwrap();
        assert!(crate::profile::Selection::from_nested(&t).include_default);
        // install = true with default-profile = false is allowed.
        let f = nested(
            "[discover]\nsources = [{ source = \"o/r\", install = true, default-profile = false }]\n",
        );
        f.validate(p).unwrap();
        assert!(!crate::profile::Selection::from_nested(&f).include_default);
        // A name with ':' (an item pattern) and an empty name are refused.
        for bad in ["skill:x", ""] {
            let n = nested(&format!(
                "[discover]\nsources = [{{ source = \"o/r\", profiles = [\"{bad}\"] }}]\n"
            ));
            assert!(
                matches!(n.validate(p), Err(MindError::MindToml { .. })),
                "{bad:?}"
            );
        }
        // One bad name among good ones is still refused; ANSI is stripped.
        let n = nested(
            "[discover]\nsources = [{ source = \"o/\\u001b[31mr\", profiles = [\"ok\", \"\\u001b[31mBad\"] }]\n",
        );
        match n.validate(p) {
            Err(MindError::MindToml { msg, .. }) => {
                assert!(!msg.contains('\u{1b}'), "{msg:?}");
                assert!(msg.contains("Bad"), "{msg}");
            }
            other => panic!("expected MindToml, got {other:?}"),
        }
        // profiles must be an array of strings: a scalar fails to parse.
        assert!(
            toml::from_str::<MindToml>(
                "[discover]\nsources = [{ source = \"o/r\", profiles = \"full\" }]\n"
            )
            .is_err()
        );
    }

    // spec: PROF-1 PROF-2
    #[test]
    fn profiles_table_load_edge_cases() {
        // An empty [profiles] table is valid and present.
        let dir = prof_dir("[profiles]\n");
        let mt = MindToml::load(&dir).unwrap().unwrap();
        assert_eq!(mt.profiles.as_ref().map(|p| p.len()), Some(0));
        assert!(!mt.is_authoritative());
        let _ = std::fs::remove_dir_all(&dir);

        // A profile with an empty member list is valid.
        let dir = prof_dir("[profiles]\nnone = []\n");
        assert!(MindToml::load(&dir).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);

        // A member that is not a string fails to parse (not a panic).
        let dir = prof_dir("[profiles]\np = [1]\n");
        assert!(MindToml::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        // A profile name over 64 bytes is a MindToml error.
        let dir = prof_dir(&format!("[profiles]\n{} = []\n", "a".repeat(65)));
        match MindToml::load(&dir) {
            Err(MindError::MindToml { msg, .. }) => assert!(msg.contains("64 bytes"), "{msg}"),
            other => panic!("expected MindToml, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        // A quoted name containing ':' is rejected as a profile name.
        let dir = prof_dir("[profiles]\n\"skill:x\" = []\n");
        match MindToml::load(&dir) {
            Err(MindError::MindToml { msg, .. }) => assert!(msg.contains("contains ':'"), "{msg}"),
            other => panic!("expected MindToml, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        // A multi-profile cycle is named through load.
        let dir = prof_dir("[profiles]\na = [\"b\"]\nb = [\"a\"]\n");
        match MindToml::load(&dir) {
            Err(MindError::MindToml { msg, .. }) => {
                assert!(msg.contains("cycle: a -> b -> a"), "{msg}")
            }
            other => panic!("expected MindToml, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: PROF-8
    #[test]
    fn item_manifest_rejects_any_profiles_shape_even_with_hooks() {
        for body in [
            "[profiles]\n",
            "profiles = []\n",
            "profiles = \"x\"\n",
            "[[hooks]]\nrun = \"true\"\n[profiles]\nb = []\n",
        ] {
            let dir = prof_dir(body);
            match ItemToml::load(&dir) {
                Err(MindError::MindToml { msg, .. }) => {
                    assert!(msg.contains("root mind.toml only"), "{body:?}: {msg}")
                }
                other => panic!("{body:?}: expected MindToml, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
