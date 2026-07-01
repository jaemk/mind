//! The optional `mind.toml` a source repo may place at its root to declare its
//! inventory explicitly, instead of relying on convention scanning.
//!
//! Everything is optional. A repo with no `mind.toml` is scanned by convention.
//! A repo with only `[source]` metadata still gets convention scanning. A repo
//! that declares `[[items]]` or `[discover]` opts out of convention scanning and
//! becomes authoritative for its own inventory.

use std::path::Path;

use serde::Deserialize;

use crate::error::{MindError, Result};
use crate::git::validate_ref_value;
use crate::source::Pin;

/// The lifecycle event a hook is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Install,
    Uninstall,
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
    /// Raw event string; validated by `resolved_hooks` ("install" | "uninstall",
    /// default "install"). Kept as a string so an unknown value is a clear
    /// mind.toml schema error rather than an opaque serde failure.
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
    /// [`crate::namespace`]). A consumer `meld --as` overrides it.
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
    /// Convention scan roots (DSC-50). When set, convention discovery scans
    /// under each listed repo-root-relative directory instead of the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    /// Flat skill layout (DSC-74). When true, convention discovery finds skills
    /// as bare-name directories with a direct `SKILL.md` under each scan root,
    /// with no `skills/` container. Default false (the DSC-10 container behavior).
    #[serde(rename = "flat-skills", default)]
    pub flat_skills: bool,
}

impl SourceMeta {
    /// Return the single pin declared by this `[source]` section, or `None`
    /// if none is declared. More than one declared pin is a `MindToml` error
    /// (DSC-41).
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
        match set.len() {
            0 => Ok(None),
            1 => Ok(Some(set.remove(0).1)),
            _ => {
                let names: Vec<&str> = set.iter().map(|(k, _)| *k).collect();
                Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "conflicting pin directives: {}; declare at most one of follow-branch, pin-tag, pin-ref",
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
    /// `skill`, `agent`, `rule`, or `tool`.
    pub kind: String,
    pub name: String,
    /// Path to the item, relative to the repo root (a dir for skills/tools).
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
    /// An item uninstall hook (HOOK-80): a shell command run when this item is
    /// removed (at `forget`/`unmeld`/rename-on-upgrade), before its store copy
    /// and links are removed. Valid on any kind. `None` means no uninstall hook.
    pub uninstall: Option<String>,
    /// Item lifecycle hooks declared as an array (HOOK-86), the per-item analog
    /// of the source `[[hooks]]` array (HOOK-50): the field is `[[items.hooks]]`
    /// in `mind.toml`. Same fields/semantics as a source hook. The scalar
    /// `install`/`uninstall` above are folded in ahead of these (see
    /// [`ItemDecl::resolved_item_hooks`]).
    #[serde(default)]
    pub hooks: Vec<Hook>,
}

impl ItemDecl {
    /// Resolve this item's lifecycle hooks (HOOK-86) in execution order.
    ///
    /// Mirrors [`MindToml::resolved_hooks`]: the scalar `install` folds in as the
    /// first required install hook and the scalar `uninstall` as the first
    /// required uninstall hook (HOOK-80 shorthand), both ahead of the
    /// `[[items.hooks]]` entries, which are then validated and appended in
    /// declaration order. An empty/whitespace `run` is dropped (HOOK-3) and an
    /// unknown `event` is a `mind.toml` schema error.
    pub fn resolved_item_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        let mut out: Vec<ResolvedHook> = Vec::new();

        // HOOK-86: fold the scalar install/uninstall shorthand in first, each as
        // a required hook of its event.
        for (scalar, event) in [
            (&self.install, HookEvent::Install),
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

    /// Resolve the `[[items.hooks]]` array entries (HOOK-86), validated in
    /// declaration order. Returns only the array-declared hooks; the scalar
    /// `install`/`uninstall` fold-in is handled by the caller
    /// (`resolved_item_hooks`), which prepends those scalars ahead of this
    /// result.
    fn resolved_item_array_hooks(&self, toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
        resolve_hook_array(&self.hooks, toml_path)
    }
}

/// Validate and normalize a slice of raw `Hook` entries into `ResolvedHook`s
/// in declaration order. Used by `MindToml::resolved_hooks`,
/// `ItemDecl::resolved_item_array_hooks`, and `NestedSource::resolved_hooks`.
///
/// Rules: default event is Install; "uninstall" maps to Uninstall; any other
/// event string is a `MindToml` error. Empty/whitespace `run` entries are
/// silently dropped (HOOK-3). `run` is trimmed before storing.
fn resolve_hook_array(hooks: &[Hook], toml_path: &std::path::Path) -> Result<Vec<ResolvedHook>> {
    let mut out: Vec<ResolvedHook> = Vec::new();
    for hook in hooks {
        let run = hook.run.trim();
        if run.is_empty() {
            continue; // HOOK-3 generalized: skip blank run
        }
        let event = match hook.event.as_deref() {
            None | Some("install") => HookEvent::Install,
            Some("uninstall") => HookEvent::Uninstall,
            Some(e) => {
                return Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!("unknown hook event '{e}'; expected 'install' or 'uninstall'"),
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
    /// Tool globs match the tool DIRECTORY (e.g. `packages/*/tool`), not an
    /// anchor file: the matched directory is the tool.
    #[serde(default)]
    pub tools: KindGlobs,
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
    #[allow(dead_code)] // consumed by callers in commands.rs once DSC-78 is wired end-to-end
    pub namespace: Option<String>,
    /// Legacy alias key kept for backwards compatibility (DSC-78). Prefer `namespace`.
    #[serde(rename = "as", default)]
    pub alias: Option<String>,
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
    /// Curator-supplied convention scan roots (DSC-59 / DSC-50 shape): when
    /// set, convention discovery scans under each listed directory instead of
    /// the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
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
    #[allow(dead_code)] // consumed by callers in commands.rs once DSC-78 is wired end-to-end
    pub fn effective_alias(&self) -> Option<String> {
        self.namespace.clone().or_else(|| self.alias.clone())
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
        Ok(())
    }

    /// Return the curator-supplied pin directive, or `None` when not set.
    ///
    /// Accepts any of `follow-branch`, `pin-tag`, or `pin-ref` (DSC-59,
    /// DSC-41 one-of rule). Declaring more than one is a `MindToml` error.
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
        match set.len() {
            0 => Ok(None),
            1 => Ok(Some(set.remove(0).1)),
            _ => {
                let names: Vec<&str> = set.iter().map(|(k, _)| *k).collect();
                Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "nested source '{}': conflicting pin directives: {}; declare at most one of follow-branch, pin-tag, pin-ref",
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
        resolve_hook_array(&self.hooks, toml_path)
    }
}

impl Discover {
    /// Whether this section declares item globs (as opposed to only nested
    /// sources). Item globs turn off convention discovery; a bare `sources` list
    /// does not.
    pub fn has_item_globs(&self) -> bool {
        !self.skills.include.is_empty()
            || !self.agents.include.is_empty()
            || !self.rules.include.is_empty()
            || !self.tools.include.is_empty()
    }
}

/// Validate that `val` is a well-formed dotted numeric version string, as
/// required for `min-mind-version` (DSC-40). A valid string is non-empty, and
/// every dot-separated component is itself non-empty and consists solely of
/// ASCII decimal digits (e.g. `"1"`, `"0.7"`, `"2.3.1"`). Returns a
/// `MindToml` error naming `field` and the bad value when validation fails.
///
/// This validator applies to the *field value* in `mind.toml`. The running
/// binary's version string (from the build environment) may carry a
/// pre-release segment (e.g. `0.2.0-rc1`); such a segment compares as 0
/// in [`version_at_least`] only.
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
/// non-numeric component compares as 0, so a prerelease/build suffix is ignored.
pub fn version_at_least(running: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.')
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

impl MindToml {
    /// Load `mind.toml` from a repo root, returning `None` if absent.
    ///
    /// Validates `[source].min-mind-version` format at parse time (DSC-40):
    /// the field, when present, must be a dotted purely-numeric version string.
    pub fn load(root: &Path) -> Result<Option<MindToml>> {
        let file = root.join("mind.toml");
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let parsed: MindToml = toml::from_str(&text).map_err(|e| MindError::Toml {
                    path: file.clone(),
                    source: e,
                })?;
                // spec: DSC-40 — validate format of min-mind-version at parse time.
                if let Some(v) = &parsed.source.min_mind_version {
                    validate_version_string(v, "min-mind-version", &file)?;
                }
                // spec: NS-25 — a declared `[source].prefix` that is a reserved
                // item-kind word is rejected at load, before it can reach the
                // effective-prefix resolution.
                if let Some(p) = &parsed.source.prefix {
                    crate::namespace::validate_prefix(p)?;
                }
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    /// Whether this file takes over item discovery (vs. leaving it to
    /// convention). Nested `[discover].sources` alone does not.
    pub fn is_authoritative(&self) -> bool {
        !self.items.is_empty() || self.discover.as_ref().is_some_and(|d| d.has_item_globs())
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
        out.extend(resolve_hook_array(&self.hooks, toml_path)?);

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
        // spec: NS-25 — a `[source].prefix` equal to a reserved item-kind word
        // (skill/agent/rule/tool) is rejected at load with ReservedPrefix, before
        // it can become an effective prefix that would alias `prefix:name` onto a
        // kind-qualified ref.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        for word in ["skill", "agent", "rule", "tool"] {
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
        // full MindToml round-trip.
        let toml = r#"
            [[discover.sources]]
            source = "github:owner/repo"
            as = "or"
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
        assert_eq!(ns.alias.as_deref(), Some("or"));
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: Some("develop".into()),
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
                alias: None,
                install: false,
                install_items: None,
                follow_branch: Some(branch.into()),
                pin_tag: None,
                pin_ref: None,
                roots: None,
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
            alias: None,
            install: true,
            install_items: Some(vec!["skill:review".into()]),
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: true,
            install_items: Some(vec![]), // empty is fine
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: Some(vec!["skill:review".into(), "agent:dev".into()]),
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: Some("--upload-pack=touch /tmp/pwned".into()),
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: Some("-evil".into()),
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("--depth=1".into()),
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: Some("main branch".into()),
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("main..HEAD".into()),
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: Some("cafebabecafebabecafebabecafebabecafebabe".into()),
            roots: None,
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
            alias: None,
            install: false,
            install_items: None,
            follow_branch: None,
            pin_tag: None,
            pin_ref: None,
            roots: None,
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
}
