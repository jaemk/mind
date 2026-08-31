//! The on-disk layout for `mind`.
//!
//! ```text
//! ~/.mind/
//!   sources.json                 registry of melded sources (see source.rs)
//!   manifest.json                installed-item manifest (see manifest.rs)
//!   sources/<host>/<owner>/<repo> bare-ish clones of each melded repo
//!   store/<kind>/<name>/          the installed copy of each item
//!
//! <agent home>/                     (one or more; default ~/.claude)
//!   skills/<name>  -> symlink into store/skill/<name>
//!   agents/<name>.md -> symlink into store/agent/<name>
//!   rules/<name>.md  -> symlink into store/rule/<name>
//! ```
//!
//! Items are linked into every configured agent home (see [`Paths::agent_homes`]).
//! Roots are overridable via environment variables so the test harness can point
//! them at temp dirs: `MIND_HOME`, `CLAUDE_HOME`, `MIND_AGENT_HOMES`.
//!
//! The default lobe (the home written into a fresh config, and the fallback when
//! no lobes are configured) is `MIND_DEFAULT_LOBE` if set, else `CLAUDE_HOME` if
//! set, else `~/.claude`. `MIND_DEFAULT_LOBE` therefore takes precedence over
//! `CLAUDE_HOME` (CLI-170); both `Paths::resolve` and `default_lobe` honor that
//! order.
//!
//! `--local` on `learn`/`meld` narrows the install fan-out to a single project
//! lobe for one invocation (HARN-20); see [`Paths::scope_to_local_lobe`] and
//! [`Paths::detect_local_lobe`].
//!
//! A lobe is the parent of `skills/` / `agents/` / `rules/`; the default is
//! `~/.claude`, but a lobe may be any harness home (Gemini, Codex, Windsurf, Antigravity)
//! because the skill/agent layouts double as the cross-tool conventions
//! (spec/harness-lobes.md). A lobe may carry a `kinds` filter (HARN-1): only
//! items of a listed kind link into it. The [`PRESETS`] table maps a harness name
//! to its lobe path and kinds (HARN-4), and [`detect_homes`] reports which preset
//! dirs exist under the detection base (HARN-5), consulting `MIND_DETECT_HOME`
//! (else the home dir) so detection stays hermetic without mutating process HOME.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::policy::Policy;

thread_local! {
    /// The active `--local` install scope (HARN-20): when set, [`Paths::agent_homes`]
    /// returns exactly this lobe instead of the global fan-out set. Thread-local
    /// (not a process global) so it needs no locking and cannot leak across the
    /// TUI's worker threads; the guard that owns it clears it on drop.
    static LOCAL_LOBE: RefCell<Option<Lobe>> = const { RefCell::new(None) };
}

fn local_lobe_override() -> Option<Lobe> {
    LOCAL_LOBE.with(|c| c.borrow().clone())
}

/// RAII guard for a `--local` install scope (HARN-20). While it is alive,
/// [`Paths::agent_homes`] returns exactly the scoped lobe; dropping it restores
/// the normal (global fan-out) resolution. Created by
/// [`Paths::scope_to_local_lobe`].
#[must_use = "dropping the guard immediately ends the --local scope"]
pub struct LocalLobeGuard(());

impl Drop for LocalLobeGuard {
    fn drop(&mut self) {
        LOCAL_LOBE.with(|c| *c.borrow_mut() = None);
    }
}

/// A resolved agent home: an absolute path plus the kinds it admits (HARN-1).
/// `kinds == None` is "no filter": it admits every kind, the historical behavior
/// (so a tool with an explicit `link`, TOOL-4, still surfaces). `Some(list)`
/// admits only the listed kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lobe {
    pub path: PathBuf,
    pub kinds: Option<Vec<ItemKind>>,
}

impl Lobe {
    /// A lobe with no kinds filter (admits all kinds).
    pub fn all_kinds(path: PathBuf) -> Self {
        Lobe { path, kinds: None }
    }

    /// Whether this lobe accepts an item of `kind` (HARN-1). With no filter every
    /// kind is admitted (preserving the pre-feature behavior, including a tool
    /// with an explicit link); with a filter, only the listed kinds.
    pub fn admits(&self, kind: ItemKind) -> bool {
        match &self.kinds {
            None => true,
            Some(kinds) => kinds.contains(&kind),
        }
    }

    /// Whether this lobe's parent directory exists (STO-56). Returns `true` when
    /// the lobe has no parent (e.g. a root path) so it is treated as always
    /// reachable. A global home like `~/.claude` always has a parent (`~`) that
    /// exists; a project lobe like `<project>/.windsurf` requires `<project>` to
    /// be present.
    ///
    /// This gate must NOT be applied inside `agent_homes()` -- uninstall
    /// confinement and `~/.claude` auto-create-on-link both depend on the full
    /// list. Apply it only at the reachability-sensitive write sites (the install
    /// fan-out's `planned_links` and `relink`). It is intentionally NOT checked at
    /// `link_into_new_lobes`: that path backfills links into a newly added lobe
    /// (e.g. a preset base such as `base/.gemini/config`) whose parent may not
    /// exist yet, so gating there would suppress the very links it exists to
    /// create (STO-56).
    // spec: STO-56
    pub(crate) fn reachable(&self) -> bool {
        self.path.parent().is_none_or(|p| p.exists())
    }
}

/// Whether a lobe is global (linked under a harness home directory) or
/// project-scoped (linked under a per-project subdirectory).
///
/// Global lobes are auto-addable by `config lobes detect` when their marker is
/// found. Project-scoped lobes require an explicit project directory; detect
/// surfaces them so the caller can print guidance, but does not auto-add them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The lobe lives under a user-wide home directory (e.g. `~/.gemini/config`).
    Global,
    /// The lobe lives under a per-project directory (e.g. `<project>/.windsurf`).
    Project,
}

/// A known harness preset (HARN-4): the lobe path (relative to the detection
/// base / home) and the kinds it admits. `marker_rel` is the on-disk signal
/// [`detect_homes`] checks to decide the harness is installed.
#[derive(Debug)]
pub struct Preset {
    /// The preset name used on the CLI (`--preset <name>`).
    pub name: &'static str,
    /// The lobe subdirectory, relative to the base directory (e.g. `.gemini/config`
    /// for Gemini). For a `Global` preset, the base is `~`; for a `Project` preset,
    /// the base is the project directory supplied by the caller.
    pub rel_path: &'static str,
    /// The kinds this preset's lobe admits.
    pub kinds: &'static [ItemKind],
    /// The directory whose presence signals this harness is installed, relative
    /// to the detection base (e.g. `.gemini` for Gemini, `.codex` for Codex,
    /// `.codeium/windsurf` for Windsurf).
    pub marker_rel: &'static str,
    /// Whether this preset is global (linked under a shared harness home) or
    /// project-scoped (linked under a per-project directory).
    pub scope: Scope,
}

/// The harness presets (HARN-4). Detection signals (HARN-5):
/// - `gemini`: `~/.gemini` exists (Gemini CLI / Antigravity shared home; lobe is `~/.gemini/config`).
/// - `codex`: `~/.codex` exists (Codex CLI's home; it reads `~/.agents`).
/// - `universal`: `~/.agents` exists (the vendor-neutral alias dir itself).
/// - `windsurf`: `~/.codeium/windsurf` exists (Windsurf IDE's real global config home;
///   lobe is `<project>/.windsurf`, project-scoped because Windsurf discovers skills
///   only at `<project>/.windsurf/skills/<name>/SKILL.md`, not from a global dir).
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "gemini",
        rel_path: ".gemini/config",
        kinds: &[ItemKind::Skill],
        marker_rel: ".gemini",
        scope: Scope::Global,
    },
    Preset {
        name: "codex",
        rel_path: ".agents",
        kinds: &[ItemKind::Skill],
        marker_rel: ".codex",
        scope: Scope::Global,
    },
    Preset {
        name: "universal",
        rel_path: ".agents",
        kinds: &[ItemKind::Skill],
        marker_rel: ".agents",
        scope: Scope::Global,
    },
    Preset {
        name: "windsurf",
        rel_path: ".windsurf",
        kinds: &[ItemKind::Skill],
        // The real Windsurf global config home is ~/.codeium/windsurf, not ~/.windsurf.
        // Detection checks for the real global home so it fires when Windsurf is installed,
        // not on an arbitrary ~/.windsurf dir. (spec: STO-56)
        marker_rel: ".codeium/windsurf",
        scope: Scope::Project,
    },
];

/// Look up a preset by name, erroring with [`MindError::UnknownPreset`] on a bad
/// name (HARN-4).
pub fn lookup_preset(name: &str) -> Result<&'static Preset> {
    PRESETS
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| MindError::UnknownPreset {
            name: name.to_string(),
        })
}

/// Resolved filesystem roots for a `mind` invocation.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.mind` (or `$MIND_HOME`).
    pub mind_home: PathBuf,
    /// `~/.claude` (or `$CLAUDE_HOME`).
    pub claude_home: PathBuf,
}

impl Paths {
    /// Resolve roots from the environment, falling back to the home directory.
    pub fn resolve() -> Result<Self> {
        let mind_home = match std::env::var_os("MIND_HOME") {
            Some(p) => PathBuf::from(p),
            None => home()?.join(".mind"),
        };
        // spec: CLI-170 - MIND_DEFAULT_LOBE takes precedence over CLAUDE_HOME.
        let claude_home = match std::env::var_os("MIND_DEFAULT_LOBE")
            .or_else(|| std::env::var_os("CLAUDE_HOME"))
        {
            Some(p) => PathBuf::from(p),
            None => home()?.join(".claude"),
        };
        Ok(Self {
            mind_home,
            claude_home,
        })
    }

    /// Path to the global advisory lock file.
    // spec: STO-40
    pub fn lock_file(&self) -> PathBuf {
        self.mind_home.join(".lock")
    }

    pub fn sources_file(&self) -> PathBuf {
        self.mind_home.join("sources.json")
    }

    pub fn manifest_file(&self) -> PathBuf {
        self.mind_home.join("manifest.json")
    }

    /// The user config file (`config.toml`) under the mind home.
    pub fn config_file(&self) -> PathBuf {
        self.mind_home.join("config.toml")
    }

    /// Root under which melded repos are cloned.
    pub fn sources_dir(&self) -> PathBuf {
        self.mind_home.join("sources")
    }

    /// Root under which installed item copies live.
    pub fn store_dir(&self) -> PathBuf {
        self.mind_home.join("store")
    }

    /// The store location for one installed item.
    pub fn store_item(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.mind_home.join(self.store_rel(kind, name))
    }

    /// The store location for one item, relative to `mind_home` (recorded in the
    /// manifest so uninstall removes exactly what was installed).
    pub fn store_rel(&self, kind: ItemKind, name: &str) -> String {
        format!("store/{}/{}", kind.as_str(), name)
    }

    /// Scratch root for transactional installs (staging + backup).
    pub fn tmp_dir(&self) -> PathBuf {
        self.mind_home.join(".tmp")
    }

    /// Where a new item copy is built before it is swapped into the store.
    pub fn staging_path(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.tmp_dir()
            .join("staging")
            .join(kind.as_str())
            .join(name)
    }

    /// Where the previous store copy is held during a swap, for rollback.
    pub fn backup_path(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.tmp_dir().join("backup").join(kind.as_str()).join(name)
    }

    /// The default link target for an item, relative to an agent home, or `None`
    /// for a kind that is store-only by default (a `tool`: it carries no symlink
    /// and the harness does not discover it; items reach it by path token).
    pub fn default_link_rel(&self, kind: ItemKind, name: &str) -> Option<String> {
        let dir = kind.dir();
        match kind {
            ItemKind::Skill => Some(format!("{dir}/{name}")),
            // spec: CMD-5 -- a command links like an agent or rule, under its
            // effective name, so a prefixed command reads as `/<prefix>:<name>`.
            ItemKind::Agent | ItemKind::Rule | ItemKind::Command => {
                Some(format!("{dir}/{name}.md"))
            }
            ItemKind::Tool => None,
        }
    }

    /// The agent homes items are linked into. Without a managed policy this is,
    /// in order: `$MIND_AGENT_HOMES` (a `:`-separated path list), else `lobes`
    /// from `~/.mind/config.toml`, else `[claude_home]`.
    ///
    /// When a managed policy is in effect:
    /// - `[lobes].lock = true` (POL-40): the effective homes are exactly
    ///   `[lobes].targets`; `$MIND_AGENT_HOMES` and config `lobes` are ignored.
    ///   An empty `targets` under a lock falls back to the default (`claude_home`).
    /// - `[lobes].lock = false` (POL-41): `[lobes].targets` is a base set that is
    ///   unioned with the user's normally-resolved homes (deduped; targets first).
    ///
    /// A leading `~` is expanded, and a relative path is resolved to absolute
    /// against the current directory, so the link paths recorded in the manifest
    /// never depend on the working directory at a later uninstall.
    ///
    /// Each returned [`Lobe`] carries its `kinds` filter (HARN-1): config entries
    /// carry the filter they declared; `$MIND_AGENT_HOMES` entries and managed
    /// policy targets resolve to `kinds: None` (all kinds), preserving current
    /// behavior. Lobes are deduplicated by path (first-seen kinds win).
    pub fn agent_homes(&self) -> Result<Vec<Lobe>> {
        // spec: HARN-20 -- an active `--local` scope narrows the fan-out to the
        // single project lobe it holds. That lobe was drawn from this same
        // effective set by `detect_local_lobe` before the scope was set (the
        // override is None during detection), so returning it here is a subset of
        // the normally-resolved homes and does not bypass managed-policy filtering.
        if let Some(lobe) = local_lobe_override() {
            return Ok(vec![lobe]);
        }
        // Compute the user's normal homes (pre-policy).
        let user_homes: Vec<Lobe> = {
            let mut h: Vec<Lobe> = Vec::new();
            if let Some(raw) = std::env::var_os("MIND_AGENT_HOMES") {
                // Env-var homes are all-kinds (HARN-2): they preserve the
                // pre-feature behavior of `$MIND_AGENT_HOMES`.
                let parsed = raw
                    .to_string_lossy()
                    .split(':')
                    .filter(|p| !p.is_empty())
                    .map(|p| Ok(Lobe::all_kinds(absolute_home(p)?)))
                    .collect::<Result<Vec<_>>>()?;
                if !parsed.is_empty() {
                    h = parsed;
                }
            }
            if h.is_empty() {
                let configured = Config::load(self)?.lobes;
                if !configured.is_empty() {
                    h = configured
                        .iter()
                        .map(|e| {
                            Ok(Lobe {
                                path: absolute_home(e.path())?,
                                kinds: e.kinds().map(<[ItemKind]>::to_vec),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                }
            }
            if h.is_empty() {
                h = vec![Lobe::all_kinds(make_absolute(self.claude_home.clone())?)];
            }
            h
        };

        // Apply managed-policy lobe rules when a policy is in effect.
        // spec: POL-40
        // spec: POL-41
        match Policy::load()? {
            Some(policy) if policy.lobes_lock() => {
                // POL-40: locked - use exactly the policy targets, ignoring user homes.
                let targets = policy.lobes_targets();
                if targets.is_empty() {
                    // Empty targets under a lock pins the default.
                    Ok(vec![Lobe::all_kinds(make_absolute(
                        self.claude_home.clone(),
                    )?)])
                } else {
                    let resolved: Vec<Lobe> = targets
                        .iter()
                        .map(|p| Ok(Lobe::all_kinds(absolute_home(p)?)))
                        .collect::<Result<_>>()?;
                    Ok(dedup_lobes(resolved))
                }
            }
            Some(policy) => {
                // POL-41: not locked - union policy targets with user homes (targets first,
                // deduped). The whole result is deduped to collapse duplicate targets and
                // targets that equal a user home.
                // spec: POL-41
                let mut result: Vec<Lobe> = Vec::new();
                for p in policy.lobes_targets() {
                    result.push(Lobe::all_kinds(absolute_home(p)?));
                }
                for h in user_homes {
                    result.push(h);
                }
                Ok(dedup_lobes(result))
            }
            None => {
                // POL-4 inert: no policy. Dedup by path (first-seen kinds win),
                // honoring the documented contract so two same-path config lobes
                // (e.g. the codex + universal presets both at ~/.agents) collapse
                // to one, exactly as the policy branches already do.
                Ok(dedup_lobes(user_homes))
            }
        }
    }

    /// The lobe a `--preset <name>` resolves to (HARN-4): the preset's lobe
    /// path (with `~` expanded to absolute, STO-16) and its kinds filter. Errors
    /// with [`MindError::UnknownPreset`] on a bad name.
    ///
    /// For `Global` presets the base is the home directory (`~`). For `Project`
    /// presets the base is the current working directory (a project-local lobe).
    pub fn preset_lobe(name: &str) -> Result<Lobe> {
        let (lobe, _preset) = resolve_lobe(None, Some(name), None)?;
        Ok(lobe)
    }

    /// Restrict the install fan-out to `lobe` for the lifetime of the returned
    /// guard (HARN-20). While the guard is held, [`Paths::agent_homes`] returns
    /// exactly `lobe`; dropping it restores the normal resolution.
    pub fn scope_to_local_lobe(lobe: Lobe) -> LocalLobeGuard {
        LOCAL_LOBE.with(|c| *c.borrow_mut() = Some(lobe));
        LocalLobeGuard(())
    }

    /// Detect the registered project lobe the current working directory sits
    /// inside (HARN-21): a configured lobe whose directory lives under the cwd
    /// (e.g. a `<project>/.windsurf` windsurf lobe, or a `<project>/<subdir>`
    /// lobe). A global home lives under `~`, not under an arbitrary project cwd,
    /// so it does not match. When several configured lobes qualify, the deepest
    /// (closest to the cwd) wins. Returns `None` when the cwd is not inside any
    /// registered project lobe, which is what makes `--local` an error there.
    ///
    /// Paths are canonicalized for the containment test so a symlinked cwd or
    /// lobe directory still matches; a path that does not exist yet falls back to
    /// its lexical form.
    ///
    /// A global home never qualifies as the project lobe an invocation is
    /// "inside", even when the containment test would otherwise match it (HARN-21:
    /// "A global home lives under `~`, not under an arbitrary project cwd, so it
    /// does not qualify"). Two things count as a global home and are excluded:
    /// - the resolved default home (`claude_home` / `MIND_DEFAULT_LOBE` /
    ///   `CLAUDE_HOME`, else `~/.claude`): it is always the global fan-out target
    ///   (HARN-20 says the default `~/.claude` is not written under `--local`), so
    ///   it can never be a project lobe. Without this, running `--local` from the
    ///   default home's PARENT (e.g. `$HOME`, with only `~/.claude` configured)
    ///   would see `~/.claude` satisfy `starts_with(cwd)` and be wrongly narrowed
    ///   to, instead of the install being refused.
    /// - any home-rooted lobe (a global preset such as `~/.gemini/config` or
    ///   `~/.agents`) when the cwd sits at or above `~`: its containment under the
    ///   cwd is then an artifact of the cwd being the home directory or an ancestor
    ///   of it, not of the lobe being a genuine project lobe.
    ///
    /// A real project lobe placed under `~` (e.g. `~/dev/proj/.windsurf` with cwd
    /// `~/dev/proj`) is unaffected: the cwd is below `~`, not at or above it, so
    /// the home-rooted exclusion does not fire.
    // spec: HARN-21
    pub fn detect_local_lobe(&self) -> Result<Option<Lobe>> {
        let cwd = std::env::current_dir().map_err(|e| MindError::io(".", e))?;
        let cwd_c = canonicalize_existing(&cwd);
        // The resolved default/global home: never a `--local` target (HARN-20/21).
        let global_home = make_absolute(self.claude_home.clone())?;
        let global_home_c = canonicalize_existing(&global_home);
        // The home directory, for the home-rooted global-preset exclusion below.
        // Absent (no home dir) means the exclusion is simply not applied.
        let home_c = home().ok().map(|h| canonicalize_existing(&h));
        let mut best: Option<(usize, Lobe)> = None;
        for lobe in self.agent_homes()? {
            // A registered project lobe often does not exist on disk yet (it is
            // created on the first link into it), so `Path::canonicalize` would
            // fail and leave the path unresolved. Resolve the deepest existing
            // ancestor instead, so a symlinked ancestor (e.g. macOS `/var` ->
            // `/private/var`) matches the likewise-resolved cwd.
            let lp = canonicalize_existing(&lobe.path);
            if lp == cwd_c || !lp.starts_with(&cwd_c) {
                continue;
            }
            // Exclude the default/global home outright (HARN-21).
            if lp == global_home_c {
                continue;
            }
            // Exclude a home-rooted global lobe when the cwd is at or above `~`:
            // the match is only an artifact of the cwd sitting at/above home.
            if let Some(home_c) = &home_c
                && lp.starts_with(home_c)
                && home_c.starts_with(&cwd_c)
            {
                continue;
            }
            let depth = lp.components().count();
            if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                best = Some((depth, lobe));
            }
        }
        Ok(best.map(|(_, l)| l))
    }

    /// The base directory detection scans under (HARN-5): `$MIND_DETECT_HOME` if
    /// set (so tests stay hermetic without mutating process HOME), else the home
    /// directory.
    pub fn detect_base() -> Result<PathBuf> {
        match std::env::var_os("MIND_DETECT_HOME") {
            Some(p) => Ok(PathBuf::from(p)),
            None => home(),
        }
    }

    /// Report which known harness preset dirs exist under the detection base
    /// (HARN-5). A preset is reported when its marker dir exists; each entry is
    /// the preset name and the [`Lobe`] (path under the base, plus kinds) to add.
    /// Detection never mutates config on its own; the caller decides.
    pub fn detect_homes() -> Result<Vec<(&'static str, Lobe)>> {
        let base = Self::detect_base()?;
        let mut found = Vec::new();
        for preset in PRESETS {
            if base.join(preset.marker_rel).is_dir() {
                found.push((
                    preset.name,
                    Lobe {
                        path: base.join(preset.rel_path),
                        kinds: Some(preset.kinds.to_vec()),
                    },
                ));
            }
        }
        Ok(found)
    }

    /// The default lobe written into a fresh config: the `$CLAUDE_HOME` override
    /// if set, else `~/.claude`.
    pub fn default_lobe(&self) -> String {
        // spec: CLI-170 - MIND_DEFAULT_LOBE takes precedence over CLAUDE_HOME.
        match std::env::var_os("MIND_DEFAULT_LOBE").or_else(|| std::env::var_os("CLAUDE_HOME")) {
            Some(v) => v.to_string_lossy().into_owned(),
            None => "~/.claude".to_string(),
        }
    }

    /// Create `config.toml` with default values if it does not exist yet.
    pub fn ensure_config(&self) -> Result<()> {
        if !self.config_file().exists() {
            Config {
                lobes: vec![crate::config::LobeEntry::bare(self.default_lobe())],
                ..Default::default()
            }
            .save(self)?;
        }
        Ok(())
    }

    /// Create the `~/.mind` scaffolding (and a default config) if absent.
    pub fn ensure_layout(&self) -> Result<()> {
        mkdir_p(&self.mind_home)?;
        mkdir_p(&self.sources_dir())?;
        mkdir_p(&self.store_dir())?;
        self.ensure_config()?;
        Ok(())
    }

    /// Write `bytes` to `target` atomically by writing a sibling temp file and
    /// renaming it over the target. Callers see either the old file or the new
    /// file, never a partial write.
    ///
    /// The temp file is placed in the same directory as `target` (required for
    /// `rename` to be atomic within one filesystem). Named
    /// `.<filename>.tmp.<pid>` so it is identifiable on crash.
    ///
    /// Called by `source.rs`, `manifest.rs`, and `config.rs` once the
    /// mechanical shard wires them up; until then the unit tests exercise it.
    // spec: STO-43
    pub fn atomic_write(target: &std::path::Path, bytes: &[u8]) -> Result<()> {
        let dir = target
            .parent()
            .ok_or_else(|| MindError::io(target, std::io::Error::other("no parent directory")))?;
        let file_name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let tmp_name = format!(".{}.tmp.{}", file_name, std::process::id());
        let tmp_path = dir.join(&tmp_name);

        // Write to temp and fsync it before the rename, so a crash after the
        // rename cannot leave the target pointing at an inode whose data never
        // reached disk (a truncated/zero-length manifest.json/sources.json). The
        // sync_all flushes both the file contents and its length. Clean up on error.
        use std::io::Write as _;
        let write_result = std::fs::File::create(&tmp_path)
            .and_then(|mut f| {
                f.write_all(bytes)?;
                f.sync_all()
            })
            .map_err(|e| MindError::io(&tmp_path, e));
        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        // Rename over the target; clean up temp on error.
        std::fs::rename(&tmp_path, target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            MindError::io(target, e)
        })
    }
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or(MindError::HomeDirNotFound)
}

/// Expand a leading `~` / `~/` to the home directory; other paths pass through.
fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(h) = dirs::home_dir()
    {
        return h.join(rest);
    }
    PathBuf::from(path)
}

/// Expand `~` and then resolve a relative agent-home path to an absolute one.
fn absolute_home(path: &str) -> Result<PathBuf> {
    make_absolute(expand_home(path))
}

/// Resolve a path to absolute against the current directory, leaving an
/// already-absolute path unchanged. Does not touch the filesystem (no symlink
/// resolution), so it works for a home that does not exist yet.
fn make_absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir().map_err(|e| MindError::io(".", e))?;
    Ok(cwd.join(path))
}

/// Canonicalize as much of `path` as exists on disk, re-appending the
/// non-existent trailing components lexically.
///
/// `Path::canonicalize` fails outright when the full path does not yet exist,
/// leaving symlinked ancestors unresolved. This resolves the deepest existing
/// ancestor (so e.g. macOS `/var` -> `/private/var` is applied) and rejoins the
/// remaining components, so containment checks compare like-for-like even for a
/// path that has not been created yet (a registered project lobe dir before its
/// first link). Falls back to the lexical path when nothing resolves.
fn canonicalize_existing(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur: &Path = path;
    loop {
        if let Ok(resolved) = cur.canonicalize() {
            let mut out = resolved;
            for name in tail.iter().rev() {
                out.push(name);
            }
            return out;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_owned());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Deduplicate a `Vec<Lobe>` by path, preserving first-seen order. When the same
/// path appears twice, the first-seen lobe (and its kinds) wins.
fn dedup_lobes(lobes: Vec<Lobe>) -> Vec<Lobe> {
    let mut seen = std::collections::HashSet::new();
    lobes
        .into_iter()
        .filter(|l| seen.insert(l.path.clone()))
        .collect()
}

/// Resolve a lobe from the combination of an optional base directory, an
/// optional harness preset name, and an optional relative subdirectory.
///
/// This is the single resolution entry point for `config lobes add` and the
/// planned `link-project` command. Three dispatch cases:
///
/// - **preset given** (`preset.is_some()`): subdir and kinds come from the
///   preset. `base` provides the root; when absent it defaults to `~` for a
///   [`Scope::Global`] preset or the cwd for a [`Scope::Project`] preset. The
///   resolved lobe path is `base / preset.rel_path`.
///
/// - **`subdir` given, no preset**: `base` defaults to the cwd when absent.
///   The resolved lobe path is `base / subdir`. kinds = `[Skill]`.
///
/// - **neither preset nor subdir** (bare path add): the lobe path is `base`
///   (required; if also absent, returns `MindError::LobeTargetRequired`).
///   kinds = None (all kinds).
///
/// In all cases the resolved path is made absolute (STO-16).
///
/// When `base` is explicitly given and that path does not exist, returns
/// `MindError::LobeBaseMissing` (STO-56). The home directory is not statted.
// spec: STO-16
// spec: STO-56
pub(crate) fn resolve_lobe(
    base: Option<&str>,
    preset: Option<&str>,
    subdir: Option<&str>,
) -> Result<(Lobe, Option<&'static Preset>)> {
    // Helper: check that an explicit base directory exists before using it.
    // We skip the stat when the expanded path is the home directory (always
    // present, and the caller may legitimately supply "~").
    let check_base = |base_str: &str| -> Result<PathBuf> {
        let expanded = expand_home(base_str);
        let home = dirs::home_dir();
        let is_home = home.as_deref().is_some_and(|h| expanded == h);
        if !is_home && !expanded.exists() {
            return Err(MindError::LobeBaseMissing { path: expanded });
        }
        make_absolute(expanded)
    };

    if let Some(name) = preset {
        // --- Case 1: preset given ---
        let p = lookup_preset(name)?;
        let base_path: PathBuf = match base {
            Some(b) => check_base(b)?,
            None => match p.scope {
                Scope::Global => make_absolute(home()?)?,
                Scope::Project => {
                    make_absolute(std::env::current_dir().map_err(|e| MindError::io(".", e))?)?
                }
            },
        };
        let lobe_path = make_absolute(base_path.join(p.rel_path))?;
        Ok((
            Lobe {
                path: lobe_path,
                kinds: Some(p.kinds.to_vec()),
            },
            Some(p),
        ))
    } else if let Some(rel) = subdir {
        // --- Case 2: subdir given, no preset ---
        let base_path: PathBuf = match base {
            Some(b) => check_base(b)?,
            None => make_absolute(std::env::current_dir().map_err(|e| MindError::io(".", e))?)?,
        };
        let lobe_path = make_absolute(base_path.join(rel))?;
        Ok((
            Lobe {
                path: lobe_path,
                kinds: Some(vec![ItemKind::Skill]),
            },
            None,
        ))
    } else {
        // --- Case 3: neither preset nor subdir (bare path add) ---
        match base {
            Some(b) => {
                // Stat the given base (it IS the lobe, not a container).
                let expanded = expand_home(b);
                let lobe_path = make_absolute(expanded)?;
                Ok((Lobe::all_kinds(lobe_path), None))
            }
            None => Err(MindError::LobeTargetRequired),
        }
    }
}

/// `mkdir -p` that tags failures with the offending path.
// spec: HARN-23
pub fn mkdir_p(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| classify_mkdir_error(path, e))
}

/// Turn a raw `create_dir_all(path)` failure into a [`MindError`], substituting
/// the [`MindError::BrokenSymlinkPath`] diagnosis only for its exact known
/// signature (HARN-23). Split out from [`mkdir_p`] so the classification rule
/// can be exercised directly with a synthetic `io::Error` (a real
/// `create_dir_all` call cannot be made to report an arbitrary error kind
/// on demand).
///
/// `create_dir_all` reports a dangling-symlink component as a bare `File
/// exists` (`ErrorKind::AlreadyExists`; the link itself exists, so the
/// directory create refuses), which names neither the link nor what it points
/// at. That is reachable through a configured agent home whose path is a
/// broken symlink. Only substitute the diagnosis for exactly that error kind:
/// any other kind -- a permission or read-only-filesystem failure at some
/// deeper component, say -- keeps its real cause rather than being
/// misreported as a broken link just because a dangling symlink happens to
/// sit elsewhere in the same path.
fn classify_mkdir_error(path: &Path, e: std::io::Error) -> MindError {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        match broken_symlink_component(path) {
            Ok(Some((link, target))) => {
                return MindError::BrokenSymlinkPath {
                    path: path.to_path_buf(),
                    link,
                    target,
                };
            }
            // The broken component was identified, but its target could not
            // be read back (`read_link` itself failed); propagate that real
            // I/O error rather than rendering an empty target.
            Err(read_err) => return read_err,
            Ok(None) => {}
        }
    }
    MindError::io(path, e)
}

/// The first component of `path` (itself included) that is a symlink whose
/// target does not resolve, paired with that target resolved against the
/// link's own parent directory for display (so a relative target like
/// `../gone` renders as an interpretable path instead of a bare relative
/// string with nothing to resolve it against). `Ok(None)` when every existing
/// component resolves cleanly (or does not exist yet, a normal to-be-created
/// component). `Err(..)` when a broken-symlink component IS found but its
/// target cannot be read back at all (`read_link` failing is itself an I/O
/// error, distinct from a target that reads back but does not exist). Used to
/// explain a `create_dir_all` failure in terms of the broken link rather than
/// the `File exists` the OS reports.
///
/// Classification of "broken" is `ErrorKind::NotFound` specifically
/// (HARN-23): following a symlink via `std::fs::metadata` can also fail with
/// e.g. `PermissionDenied` or a symlink-loop error, neither of which means
/// "the target does not exist" -- misreporting either as a dangling link
/// would assert something false about a path that may well exist. A
/// non-`NotFound` failure is not treated as this component being broken; the
/// scan continues past it in case a later component is the genuine dangling
/// link.
fn broken_symlink_component(path: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        // `symlink_metadata` succeeds on a dangling link; `metadata` follows it
        // and fails. That pair is exactly the broken-symlink signature.
        let Ok(md) = std::fs::symlink_metadata(&prefix) else {
            continue; // does not exist yet: a normal to-be-created component
        };
        if !md.file_type().is_symlink() {
            continue;
        }
        let Err(follow_err) = std::fs::metadata(&prefix) else {
            continue; // the symlink resolves fine: not the broken component
        };
        if follow_err.kind() != std::io::ErrorKind::NotFound {
            // Exists but is unreadable for some other reason (permission, a
            // symlink loop, ...): not "does not exist", so don't misreport it.
            continue;
        }
        return resolve_broken_link_display(&prefix, std::fs::read_link(&prefix)).map(Some);
    }
    Ok(None)
}

/// Turn a `read_link` result for a confirmed-broken symlink at `prefix` into
/// either the display-resolved `(link, target)` pair, or the I/O error
/// `read_link` itself produced (HARN-23). A `read_link` failure is a genuine
/// I/O error distinct from "the target does not exist" (which is what got
/// `prefix` classified as broken in the first place), so it propagates rather
/// than the caller rendering an empty target. A relative target is resolved
/// against `prefix`'s own parent directory before being returned, so it
/// displays as an interpretable path (e.g. `<parent>/../gone`) instead of a
/// bare relative string with nothing to resolve it against; an absolute
/// target is returned unchanged. Split out from [`broken_symlink_component`]
/// so this step can be exercised directly with a synthetic `read_link`
/// result -- a real dangling symlink's `read_link` call essentially cannot be
/// made to fail on demand once `symlink_metadata`/`metadata` already
/// succeeded/failed as required.
fn resolve_broken_link_display(
    prefix: &Path,
    target: std::io::Result<PathBuf>,
) -> Result<(PathBuf, PathBuf)> {
    let target = target.map_err(|e| MindError::io(prefix, e))?;
    let display_target = if target.is_absolute() {
        target
    } else {
        prefix
            .parent()
            .map(|parent| parent.join(&target))
            .unwrap_or(target)
    };
    Ok((prefix.to_path_buf(), display_target))
}

/// Crate-wide lock serializing every test (in any module) that mutates a
/// process-global environment variable (`MIND_POLICY_FILE`, `MIND_AGENT_HOMES`,
/// `MIND_HOME`, `CLAUDE_HOME`, `GITHUB_TOKEN`, `GH_TOKEN`, `PATH`, ...).
///
/// `std::env::set_var`/`remove_var` soundness is process-wide, not
/// module-wide: this crate's test binary runs `src/paths.rs`, `src/commands.rs`,
/// and `src/selfupdate.rs` tests concurrently on multiple threads, and several
/// of them spawn a real `git`/`curl` child process, which snapshots `environ` at
/// spawn time. A per-module lock only protects a module's tests against
/// themselves, not against a concurrently running test in a different module
/// that is also mutating env vars — that is unsound in the general case even
/// though it has not been observed to misbehave. Hoisting a single lock here
/// (rather than one static per module) makes every env-mutating test in the
/// crate serialize against every other one. Test-only (`#[cfg(test)]`): it must
/// never be reachable from non-test code.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A command links like an agent or rule (one `.md` under the kind
    /// directory), under its EFFECTIVE name, so a prefixed command reads as
    /// `/<prefix>:<name>` at the harness prompt.
    #[test]
    fn command_links_under_commands_with_its_effective_name() {
        // spec: CMD-5 CMD-6
        let paths = Paths {
            mind_home: PathBuf::from("/mind"),
            claude_home: PathBuf::from("/claude"),
        };
        assert_eq!(
            paths
                .default_link_rel(ItemKind::Command, "review")
                .as_deref(),
            Some("commands/review.md")
        );
        assert_eq!(
            paths
                .default_link_rel(ItemKind::Command, "jk:review")
                .as_deref(),
            Some("commands/jk:review.md")
        );
        assert_eq!(
            paths.store_rel(ItemKind::Command, "jk:review"),
            "store/command/jk:review"
        );
    }

    #[test]
    fn absolute_home_resolves_relative_paths() {
        // spec: STO-16
        let abs = absolute_home("rellobe").unwrap();
        assert!(
            abs.is_absolute(),
            "relative lobe should become absolute: {abs:?}"
        );
        assert!(abs.ends_with("rellobe"));
        // An already-absolute path is unchanged.
        assert_eq!(
            absolute_home("/tmp/lobe").unwrap(),
            PathBuf::from("/tmp/lobe")
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_existing_resolves_a_symlinked_ancestor_with_a_missing_tail() {
        // spec: HARN-21
        // A registered project lobe dir often does not exist yet, and its parent
        // may be reached through a symlink (as on macOS, where the temp dir sits
        // under /var -> /private/var). canonicalize_existing must resolve the
        // existing ancestor and keep the not-yet-created tail, so the containment
        // check in detect_local_lobe compares like-for-like.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("mind-canon-{}-{n}", std::process::id()));
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // A path through the symlink whose final component does not exist.
        let resolved = canonicalize_existing(&link.join("locallobe"));
        let expected = real.canonicalize().unwrap().join("locallobe");
        assert_eq!(
            resolved, expected,
            "the symlinked ancestor should resolve and the missing tail be kept"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn mkdir_p_names_a_dangling_symlink_instead_of_reporting_file_exists() {
        // spec: HARN-22 -- a configured agent home whose path is a broken
        // symlink otherwise fails at link time with a bare `File exists`
        // (os error 17): `create_dir_all` refuses because the LINK exists, which
        // names neither the link nor its missing target. The error must identify
        // the broken component instead.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("mind-brokenlink-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("brokenlobe");
        let missing = root.join("gone");
        std::os::unix::fs::symlink(&missing, &link).unwrap();

        let err = mkdir_p(&link.join("skills")).expect_err("a dangling-symlink parent must fail");
        assert!(
            matches!(err, MindError::BrokenSymlinkPath { .. }),
            "expected BrokenSymlinkPath, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("brokenlobe") && msg.contains("gone"),
            "the message must name both the broken link and its target: {msg}"
        );
        assert!(
            !msg.contains("os error 17"),
            "the raw File-exists errno must not be the reported cause: {msg}"
        );
        assert_eq!(err.kind(), "broken-symlink-path");
        // HARN-23(a): the remedy names a real, runnable command --
        // `ConfigCmd::Lobes` requires a subcommand, so a bare `mind config
        // lobes` exits with a clap usage error.
        assert!(
            msg.contains("mind config lobes list"),
            "the remedy must name the runnable `mind config lobes list`, not a bare \
             `mind config lobes`: {msg}"
        );

        // A healthy path is unaffected: still created, still Ok.
        let good = root.join("real").join("skills");
        assert!(
            mkdir_p(&good).is_ok(),
            "a normal mkdir_p must still succeed"
        );
        assert!(good.is_dir());

        std::fs::remove_dir_all(&root).ok();
    }

    /// HARN-23(b): a symlink whose target cannot be followed for a reason
    /// OTHER than "does not exist" -- here, a self-referential loop (ELOOP) --
    /// must not be classified as a dangling/broken link: `broken_symlink_component`
    /// must return `Ok(None)`, not `Ok(Some(..))`, since "the target does not
    /// exist" is false for a loop (the target chain exists, it just never
    /// terminates). Fails against the unfixed `metadata(&prefix).is_err()`
    /// check, which treats ELOOP identically to NotFound.
    #[cfg(unix)]
    #[test]
    fn broken_symlink_component_does_not_misclassify_a_symlink_loop_as_dangling() {
        // spec: HARN-23
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("mind-brokenlink-loop-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let loop_link = root.join("loopy");
        std::os::unix::fs::symlink(&loop_link, &loop_link).unwrap();

        // Confirm this test's premise: following the loop must fail, but NOT
        // with `NotFound` -- otherwise this would not distinguish the fix from
        // the unfixed code at all.
        let follow_err = std::fs::metadata(&loop_link)
            .expect_err("a self-referential symlink must fail to resolve");
        assert_ne!(
            follow_err.kind(),
            std::io::ErrorKind::NotFound,
            "premise of this test: a symlink loop must not present as NotFound: {follow_err:?}"
        );

        let result = broken_symlink_component(&loop_link).unwrap();
        assert!(
            result.is_none(),
            "a symlink loop (not NotFound) must not be classified as a broken/dangling link: \
             {result:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// HARN-23(c): `classify_mkdir_error` must only substitute the
    /// broken-symlink diagnosis when the underlying `create_dir_all` error
    /// kind is exactly `AlreadyExists`. Even when a real dangling symlink IS
    /// present in `path`'s own ancestry (so `broken_symlink_component` would
    /// find it), a DIFFERENT outer error kind (e.g. `PermissionDenied`) must
    /// propagate as a plain `Io` error, not be reclassified as
    /// `BrokenSymlinkPath`. Fails against the unfixed code, which calls
    /// `broken_symlink_component` unconditionally on every `create_dir_all`
    /// failure regardless of its kind.
    #[cfg(unix)]
    #[test]
    fn classify_mkdir_error_does_not_reclassify_a_non_already_exists_error() {
        // spec: HARN-23
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("mind-mkdirp-guard-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("brokenlobe");
        let missing = root.join("gone");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        let target = link.join("skills");

        // Sanity: `broken_symlink_component` really does find the dangling
        // link along this exact path (otherwise the guard below is untested).
        assert!(
            broken_symlink_component(&target).unwrap().is_some(),
            "test setup: the dangling symlink must be detected along this path"
        );

        // A synthetic PermissionDenied, standing in for a real deeper I/O
        // failure that happens to share a path prefix with an unrelated
        // dangling symlink -- must NOT be reclassified.
        let synthetic = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let classified = classify_mkdir_error(&target, synthetic);
        assert!(
            !matches!(classified, MindError::BrokenSymlinkPath { .. }),
            "a PermissionDenied error must not be reclassified as BrokenSymlinkPath: \
             {classified:?}"
        );
        assert!(
            matches!(classified, MindError::Io { .. }),
            "expected a plain Io error, got {classified:?}"
        );

        // The real AlreadyExists signature IS still reclassified (control:
        // proves the guard is selective, not simply disabled).
        let real_already_exists = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "eexist");
        let classified = classify_mkdir_error(&target, real_already_exists);
        assert!(
            matches!(classified, MindError::BrokenSymlinkPath { .. }),
            "an AlreadyExists error with a genuine dangling-symlink ancestor must still be \
             reclassified: {classified:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// HARN-23(d): when the broken-symlink component's target cannot be read
    /// back at all (`read_link` itself fails), that I/O error must propagate
    /// rather than the caller substituting an empty target. Fails against the
    /// unfixed `unwrap_or_default()`, which would render `pointing at ''`.
    #[test]
    fn resolve_broken_link_display_propagates_a_read_link_failure() {
        // spec: HARN-23
        let prefix = PathBuf::from("/some/broken/link");
        let synthetic = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = resolve_broken_link_display(&prefix, Err(synthetic)).unwrap_err();
        assert!(
            matches!(err, MindError::Io { .. }),
            "a read_link failure must propagate as an Io error, not be swallowed: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("/some/broken/link"),
            "the Io error must be tagged with the offending path: {msg}"
        );
    }

    /// HARN-23(e): a relative link target is resolved against the link's own
    /// parent directory before being returned, so it displays as an
    /// interpretable path rather than a bare relative string with nothing to
    /// resolve it against. An absolute target is left unchanged.
    #[test]
    fn resolve_broken_link_display_resolves_a_relative_target_against_the_links_parent() {
        // spec: HARN-23
        let prefix = PathBuf::from("/home/user/.claude/brokenlobe");
        let (link, target) =
            resolve_broken_link_display(&prefix, Ok(PathBuf::from("../gone"))).unwrap();
        assert_eq!(link, prefix);
        assert_eq!(
            target,
            PathBuf::from("/home/user/.claude/../gone"),
            "a relative target must be joined against the link's own parent directory: {target:?}"
        );

        // An absolute target passes through unchanged.
        let (_, target) =
            resolve_broken_link_display(&prefix, Ok(PathBuf::from("/elsewhere/gone"))).unwrap();
        assert_eq!(target, PathBuf::from("/elsewhere/gone"));
    }

    #[test]
    fn atomic_write_replaces_target_content() {
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-paths-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("data.json");

        // Write initial content.
        std::fs::write(&target, b"old").unwrap();

        // Atomically replace with new content.
        Paths::atomic_write(&target, b"new").unwrap();
        let got = std::fs::read(&target).unwrap();
        assert_eq!(got, b"new", "target should contain the new bytes");

        // No temp file should be left behind.
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftover.is_empty(),
            "temp file was not cleaned up: {leftover:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_creates_target_if_absent() {
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-create-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("new.json");

        assert!(!target.exists(), "sanity: target should not exist yet");
        Paths::atomic_write(&target, b"{\"x\":1}").unwrap();
        assert!(target.exists(), "atomic_write should create the target");
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"x\":1}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_errors_and_leaves_no_temp_when_dir_is_missing() {
        // If the target's parent directory does not exist, the temp write fails.
        // atomic_write must return an Io error (not panic) and must not leave a
        // stray temp file behind (there is nowhere to leave it, but the cleanup
        // path must run without error).
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let missing_dir =
            std::env::temp_dir().join(format!("mind-paths-missing-{}-{n}", std::process::id()));
        // Deliberately do NOT create missing_dir.
        let target = missing_dir.join("data.json");

        let result = Paths::atomic_write(&target, b"data");
        match result {
            Err(MindError::Io { .. }) => {}
            other => panic!("expected Io error for missing parent dir, got {other:?}"),
        }
        assert!(
            !target.exists(),
            "target must not exist after a failed atomic_write"
        );
        assert!(
            !missing_dir.exists(),
            "atomic_write must not create the parent directory"
        );
    }

    #[test]
    fn atomic_write_preserves_existing_target_on_write_failure() {
        // A crash/error mid-write must leave the previous file intact (STO-43:
        // "a crash mid-write leaves the previous file intact"). We force the temp
        // write to fail by making the target a path under a *file* (so the temp's
        // parent is not a directory), and assert the original target is unchanged.
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-failkeep-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // `blocker` is a regular file; treating it as a directory makes any write
        // under it fail (ENOTDIR), exercising the temp-write error branch.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"i am a file").unwrap();
        let target = blocker.join("data.json");

        let result = Paths::atomic_write(&target, b"new");
        assert!(
            matches!(result, Err(MindError::Io { .. })),
            "expected Io error when temp parent is a file, got {result:?}"
        );
        // The blocker file must be untouched (not clobbered by a temp file name).
        assert_eq!(
            std::fs::read(&blocker).unwrap(),
            b"i am a file",
            "unrelated sibling content must be preserved on failure"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_to_pathless_target_is_an_error() {
        // A target with no parent (the filesystem root) cannot host a sibling
        // temp; atomic_write must surface that as an Io error, not panic.
        // spec: STO-43
        let result = Paths::atomic_write(std::path::Path::new("/"), b"x");
        assert!(
            matches!(result, Err(MindError::Io { .. })),
            "writing to a target with no usable parent must be an Io error, got {result:?}"
        );
    }

    #[test]
    fn atomic_write_uses_same_directory_for_temp() {
        // The temp file must be in the same directory as the target so rename
        // is atomic (same filesystem).
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-samedir-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("sources.json");

        // Hook: after write the temp file should be in the same directory.
        // We can verify by checking rename succeeded (no EXDEV cross-device error).
        Paths::atomic_write(&target, b"[]").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"[]");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- managed-policy lobe tests -----------------------------------------

    /// Write a policy.toml, build a Paths pointing at a temp dir, and set
    /// MIND_POLICY_FILE. Returns (Paths, managed-dir, policy-file-path, guard).
    /// The guard must be held for the duration of the test; drop it last to
    /// restore the env var.
    fn setup_policy_test(
        policy_toml: &str,
    ) -> (Paths, PathBuf, PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-policy-lobe-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();
        let policy_file = base.join("policy.toml");
        std::fs::write(&policy_file, policy_toml).unwrap();
        // Unset MIND_AGENT_HOMES so it doesn't bleed in from the outer env.
        // SAFETY: ENV_LOCK is held, so no concurrent env reads on other threads.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
            std::env::set_var("MIND_POLICY_FILE", &policy_file);
        }
        let paths = Paths {
            mind_home,
            claude_home,
        };
        (paths, base, policy_file, guard)
    }

    // POL-40: with lobes.lock=true and explicit targets, agent_homes returns
    // exactly the policy targets, ignoring $MIND_AGENT_HOMES and config lobes.
    #[test]
    fn pol40_lock_true_uses_exactly_policy_targets() {
        // spec: POL-40
        let managed = {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            std::env::temp_dir().join(format!("mind-managed-lobe-{}-{n}", std::process::id()))
        };
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{managed}\"]\n",
            managed = managed.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Write a config with a different lobe - it must be ignored under lock.
        let other_lobe = base.join("other-lobe");
        let config_toml = format!(
            "lobes = [\"{other_lobe}\"]\n",
            other_lobe = other_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        // Also set MIND_AGENT_HOMES to yet another path - also must be ignored.
        let env_lobe = base.join("env-lobe");
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", env_lobe.to_str().unwrap());
        }

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();

        // Restore env before any asserts that might panic.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert_eq!(
            homes,
            vec![managed.clone()],
            "POL-40: locked policy must return exactly the managed target, not config/env homes"
        );
        assert!(
            !homes.contains(&other_lobe),
            "config lobe must be ignored under lock"
        );
        assert!(
            !homes.contains(&env_lobe),
            "MIND_AGENT_HOMES must be ignored under lock"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: with lobes.lock=true and empty targets, agent_homes falls back to
    // the default (claude_home), not an empty list.
    #[test]
    fn pol40_lock_true_empty_targets_falls_back_to_default() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = []\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(
            homes,
            vec![paths.claude_home.clone()],
            "POL-40: empty targets under a lock must fall back to the default (claude_home)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-41: with lobes.lock=false (or absent) and policy targets set, agent_homes
    // returns the union of policy targets and user homes, with targets first and
    // no duplicates.
    #[test]
    fn pol41_lock_false_unions_policy_and_user_homes() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir = std::env::temp_dir().join(format!("mind-pol41-{}-{n}", std::process::id()));
        let policy_target = base_dir.join("policy-base");
        let user_lobe = base_dir.join("user-lobe");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{policy_target}\"]\n",
            policy_target = policy_target.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Write a config with a user lobe.
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert!(
            homes.contains(&policy_target),
            "POL-41: policy target must be present in union: {homes:?}"
        );
        assert!(
            homes.contains(&user_lobe),
            "POL-41: user lobe must also be present: {homes:?}"
        );
        // Policy target is first.
        assert_eq!(
            homes[0], policy_target,
            "POL-41: policy target must come first in the union"
        );
        // No duplicates.
        let deduped: Vec<_> = {
            let mut seen = std::collections::HashSet::new();
            homes.iter().filter(|h| seen.insert(*h)).cloned().collect()
        };
        assert_eq!(homes, deduped, "POL-41: result must not contain duplicates");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: when a policy target is already in the user's homes, it is not
    // duplicated in the result.
    #[test]
    fn pol41_deduplicates_overlapping_target_and_user_home() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let shared =
            std::env::temp_dir().join(format!("mind-pol41-shared-{}-{n}", std::process::id()));
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{shared}\"]\n",
            shared = shared.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User config also lists the same path.
        let config_toml = format!("lobes = [\"{shared}\"]\n", shared = shared.display());
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(
            homes.len(),
            1,
            "POL-41: identical target + user lobe must be deduped to one entry: {homes:?}"
        );
        assert_eq!(homes[0], shared);

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-4 inert: with no MIND_POLICY_FILE set and no system policy file,
    // agent_homes behaves exactly as before the policy feature (uses config lobes).
    #[test]
    fn pol4_inert_no_policy_uses_user_config() {
        // spec: POL-40
        // spec: POL-41
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-pol4-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        // Ensure no policy env var is set.
        // SAFETY: ENV_LOCK is held, so no concurrent env reads on other threads.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        let user_lobe = base.join("user-lobe");
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(mind_home.join("config.toml"), &config_toml).unwrap();

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(
            homes,
            vec![user_lobe.clone()],
            "POL-4 inert: without a policy, user config lobes must be used as-is"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- gap-closing managed-policy lobe tests -----------------------------

    // POL-41: the unlocked union must draw the user's homes from the
    // $MIND_AGENT_HOMES source, not only from config lobes. With an unlocked
    // policy target and MIND_AGENT_HOMES set (no config), the env home appears in
    // the union, after the policy target, with no duplicates.
    #[test]
    fn pol41_unions_policy_with_env_agent_homes() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-env-{}-{n}", std::process::id()));
        let policy_target = base_dir.join("policy-base");
        let env_lobe = base_dir.join("env-lobe");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{policy_target}\"]\n",
            policy_target = policy_target.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Drive user homes via the env var (no config.toml written), to exercise
        // the $MIND_AGENT_HOMES source of user homes specifically.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", env_lobe.to_str().unwrap());
        }

        let homes = paths.agent_homes();

        // Restore env before any asserts that might panic.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes: Vec<PathBuf> = homes.unwrap().into_iter().map(|l| l.path).collect();

        assert_eq!(
            homes,
            vec![policy_target.clone(), env_lobe.clone()],
            "POL-41: unlocked union must be [policy target, env home], targets first, deduped"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-40: a lock with MULTIPLE targets returns exactly those targets in
    // declaration order, even when several user homes are set via env (all
    // ignored under the lock).
    #[test]
    fn pol40_lock_true_multiple_targets_in_order_ignores_user_homes() {
        // spec: POL-40
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol40-multi-{}-{n}", std::process::id()));
        let t1 = base_dir.join("target-a");
        let t2 = base_dir.join("target-b");
        let t3 = base_dir.join("target-c");
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{t1}\", \"{t2}\", \"{t3}\"]\n",
            t1 = t1.display(),
            t2 = t2.display(),
            t3 = t3.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Multiple user homes via env - all must be ignored under the lock.
        let e1 = base_dir.join("env-1");
        let e2 = base_dir.join("env-2");
        let env_val = format!("{}:{}", e1.display(), e2.display());
        // Also write a config lobe to confirm config is ignored too.
        let cfg_lobe = base_dir.join("cfg-lobe");
        std::fs::write(
            paths.mind_home.join("config.toml"),
            format!("lobes = [\"{}\"]\n", cfg_lobe.display()),
        )
        .unwrap();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes: Vec<PathBuf> = homes.unwrap().into_iter().map(|l| l.path).collect();

        assert_eq!(
            homes,
            vec![t1.clone(), t2.clone(), t3.clone()],
            "POL-40: locked policy must return exactly the targets in order"
        );
        assert!(!homes.contains(&e1) && !homes.contains(&e2));
        assert!(!homes.contains(&cfg_lobe));

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: unlocked union with MULTIPLE targets and MULTIPLE user homes, where
    // one user home duplicates a policy target. Asserts the exact deduped order:
    // all targets first (in order), then the user homes not already present (in
    // order), with the overlap dropped.
    #[test]
    fn pol41_multiple_targets_and_homes_exact_deduped_order() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-multi-{}-{n}", std::process::id()));
        let t1 = base_dir.join("t1");
        let t2 = base_dir.join("t2"); // also a user home (overlap)
        let u_extra = base_dir.join("u-extra");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{t1}\", \"{t2}\"]\n",
            t1 = t1.display(),
            t2 = t2.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User homes (via env): t2 (overlaps target) then u_extra. Order matters:
        // t2 must be dropped as a dup, u_extra kept and appended last.
        let env_val = format!("{}:{}", t2.display(), u_extra.display());
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes: Vec<PathBuf> = homes.unwrap().into_iter().map(|l| l.path).collect();

        assert_eq!(
            homes,
            vec![t1.clone(), t2.clone(), u_extra.clone()],
            "POL-41: targets first in order, then non-duplicate user homes; overlap dropped"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-4 inert via the $MIND_AGENT_HOMES source (not config): with no policy
    // and MIND_AGENT_HOMES set, agent_homes returns those homes unchanged.
    #[test]
    fn pol4_inert_no_policy_uses_env_agent_homes_unchanged() {
        // spec: POL-40
        // spec: POL-41
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-pol4-env-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        let env1 = base.join("env-home-1");
        let env2 = base.join("env-home-2");
        let env_val = format!("{}:{}", env1.display(), env2.display());
        // No policy file; env drives user homes.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes: Vec<PathBuf> = homes.unwrap().into_iter().map(|l| l.path).collect();

        assert_eq!(
            homes,
            vec![env1.clone(), env2.clone()],
            "POL-4 inert: without a policy, $MIND_AGENT_HOMES homes must be returned unchanged"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: a locked target written with a leading `~` is expanded to the home
    // directory and resolved to an absolute path (via absolute_home), so the
    // effective home never depends on the working directory.
    #[test]
    fn pol40_lock_target_tilde_is_expanded_to_absolute() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = [\"~/.claude-managed\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(homes.len(), 1);
        let got = &homes[0];
        assert!(
            got.is_absolute(),
            "tilde target must resolve absolute: {got:?}"
        );
        assert!(
            got.ends_with(".claude-managed"),
            "tilde target must expand under home: {got:?}"
        );
        let home = dirs::home_dir().expect("home dir for tilde expansion");
        assert_eq!(
            got,
            &home.join(".claude-managed"),
            "POL-40: `~` target must expand to <home>/.claude-managed"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: a RELATIVE locked target is resolved to an absolute path against the
    // current directory (make_absolute), so a later uninstall sees a stable,
    // cwd-independent home rather than the verbatim relative string.
    #[test]
    fn pol40_lock_relative_target_becomes_absolute() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = [\"managed-rel-lobe\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(homes.len(), 1);
        let got = &homes[0];
        assert!(
            got.is_absolute(),
            "POL-40: a relative target must be resolved to absolute: {got:?}"
        );
        assert!(
            got.ends_with("managed-rel-lobe"),
            "the relative component must be preserved: {got:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-41: an unlocked relative target is likewise resolved to absolute before
    // the union, so the targets-first entry is a stable absolute path.
    #[test]
    fn pol41_unlocked_relative_target_becomes_absolute() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-rel-{}-{n}", std::process::id()));
        let user_lobe = base_dir.join("user-lobe");
        let policy_toml = "[lobes]\nlock = false\ntargets = [\"unlocked-rel-lobe\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", user_lobe.to_str().unwrap());
        }
        let homes = paths.agent_homes();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes: Vec<PathBuf> = homes.unwrap().into_iter().map(|l| l.path).collect();

        assert_eq!(homes.len(), 2, "target + one user home: {homes:?}");
        assert!(
            homes[0].is_absolute() && homes[0].ends_with("unlocked-rel-lobe"),
            "POL-41: unlocked relative target must resolve absolute, first: {homes:?}"
        );
        assert_eq!(homes[1], user_lobe, "user home follows the target");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: duplicate entries within `targets` itself (e.g. the policy TOML
    // lists the same path twice) collapse to a single entry. A target that
    // duplicates the user home is also collapsed. The deduped result preserves
    // first-seen order.
    #[test]
    fn pol41_duplicate_targets_collapse_to_one_entry() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-dup-{}-{n}", std::process::id()));
        let dup_target = base_dir.join("dup-target");
        let user_lobe = base_dir.join("user-lobe");

        // targets has dup_target listed twice.
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{dup}\", \"{dup}\"]\n",
            dup = dup_target.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User home is distinct from the duplicated target.
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();

        // dup_target appears only once (duplicate within targets collapsed), then user_lobe.
        assert_eq!(
            homes,
            vec![dup_target.clone(), user_lobe.clone()],
            "POL-41: duplicate targets must collapse to one entry, user home follows: {homes:?}"
        );

        // Also verify: duplicate target that also equals the user home collapses to one.
        let shared = base_dir.join("shared");
        let policy_toml2 = format!(
            "[lobes]\nlock = false\ntargets = [\"{shared}\", \"{shared}\"]\n",
            shared = shared.display(),
        );
        std::fs::write(base.join("policy.toml"), &policy_toml2).unwrap();
        let config_toml2 = format!("lobes = [\"{shared}\"]\n", shared = shared.display());
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml2).unwrap();

        let homes2: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();
        assert_eq!(
            homes2,
            vec![shared.clone()],
            "POL-41: duplicates across targets and user home must all collapse to one: {homes2:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-40: duplicate entries within `targets` in a LOCKED policy collapse to
    // a single entry. The dedup must apply in the locked branch too.
    #[test]
    fn pol40_duplicate_targets_collapse_to_one_entry() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol40-dup-{}-{n}", std::process::id()));
        let dup_target = base_dir.join("dup-locked");

        // targets has the same path twice under a lock.
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{dup}\", \"{dup}\"]\n",
            dup = dup_target.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        let homes: Vec<PathBuf> = paths
            .agent_homes()
            .unwrap()
            .into_iter()
            .map(|l| l.path)
            .collect();

        assert_eq!(
            homes,
            vec![dup_target.clone()],
            "POL-40: duplicate locked targets must collapse to one entry: {homes:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // ---- HARN: kinds filter, presets, and detection ------------------------

    // HARN-1: a lobe with no kinds filter admits every kind (no filter), while a
    // filtered lobe admits only the listed kinds.
    #[test]
    fn lobe_admits_respects_kinds_filter() {
        // spec: HARN-1
        let all = Lobe::all_kinds(PathBuf::from("/x"));
        assert!(all.admits(ItemKind::Skill));
        assert!(all.admits(ItemKind::Agent));
        assert!(all.admits(ItemKind::Rule));
        assert!(
            all.admits(ItemKind::Tool),
            "an unfiltered lobe admits all kinds, so a tool with an explicit link surfaces (TOOL-4)"
        );

        let skills_only = Lobe {
            path: PathBuf::from("/y"),
            kinds: Some(vec![ItemKind::Skill]),
        };
        assert!(skills_only.admits(ItemKind::Skill));
        assert!(
            !skills_only.admits(ItemKind::Agent),
            "a skill-only lobe must reject an agent (HARN-1)"
        );
        assert!(
            !skills_only.admits(ItemKind::Rule),
            "a skill-only lobe must reject a rule (HARN-3: rules are Claude-only)"
        );
    }

    // HARN-4: each named preset resolves to its parent path and kinds; an unknown
    // name errors with UnknownPreset. The `kinds` assertions below are also the
    // regression test for CMD-7: every preset admits skills only, so a command
    // never links into a preset lobe.
    #[test]
    fn preset_lookup_and_resolution() {
        // spec: HARN-4 CMD-7
        let gemini = lookup_preset("gemini").unwrap();
        assert_eq!(gemini.rel_path, ".gemini/config");
        assert_eq!(gemini.kinds, &[ItemKind::Skill]);
        assert_eq!(gemini.scope, Scope::Global);

        let codex = lookup_preset("codex").unwrap();
        assert_eq!(codex.rel_path, ".agents");
        assert_eq!(codex.kinds, &[ItemKind::Skill]);
        assert_eq!(codex.scope, Scope::Global);

        assert_eq!(lookup_preset("universal").unwrap().rel_path, ".agents");
        assert_eq!(lookup_preset("universal").unwrap().scope, Scope::Global);

        let windsurf = lookup_preset("windsurf").unwrap();
        assert_eq!(windsurf.rel_path, ".windsurf");
        assert_eq!(windsurf.kinds, &[ItemKind::Skill]);
        // Windsurf is project-scoped: it discovers skills only per-project.
        assert_eq!(windsurf.scope, Scope::Project);
        // Windsurf's detection marker is the real global config home, not .windsurf.
        assert_eq!(windsurf.marker_rel, ".codeium/windsurf");

        // preset_lobe("windsurf") returns the cwd/.windsurf lobe (Project scope,
        // no explicit base -> defaults to cwd).
        let ws_lobe = Paths::preset_lobe("windsurf").unwrap();
        assert!(
            ws_lobe.path.is_absolute(),
            "preset path must be absolute (STO-16)"
        );
        assert!(ws_lobe.path.ends_with(".windsurf"));
        assert_eq!(ws_lobe.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));

        // Removed presets are unknown.
        assert!(matches!(
            lookup_preset("antigravity"),
            Err(MindError::UnknownPreset { .. })
        ));
        assert!(matches!(
            lookup_preset("antigravity-cli"),
            Err(MindError::UnknownPreset { .. })
        ));

        // An unknown preset name is a structured error.
        assert!(matches!(
            lookup_preset("emacs"),
            Err(MindError::UnknownPreset { .. })
        ));

        // preset_lobe resolves the path to absolute and carries the kinds.
        let lobe = Paths::preset_lobe("gemini").unwrap();
        assert!(
            lobe.path.is_absolute(),
            "preset path must be absolute (STO-16)"
        );
        assert!(lobe.path.ends_with(".gemini/config"));
        assert_eq!(lobe.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));
        assert!(Paths::preset_lobe("nope").is_err());
    }

    // HARN-5: detect_homes reports a preset only when its marker dir exists under
    // the detection base ($MIND_DETECT_HOME), and reports the lobe under that base.
    // Windsurf's marker changed from `.windsurf` to `.codeium/windsurf` (the real
    // Windsurf global config home); its lobe subdir stays `.windsurf`.
    #[test]
    fn detect_homes_reports_existing_marker_dirs() {
        // spec: HARN-5
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-detect-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Create .gemini and .agents but NOT .codex/.gemini/config.
        // Windsurf's detection marker is now .codeium/windsurf (not .windsurf).
        std::fs::create_dir_all(base.join(".gemini")).unwrap();
        std::fs::create_dir_all(base.join(".agents")).unwrap();
        std::fs::create_dir_all(base.join(".codeium/windsurf")).unwrap();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_DETECT_HOME", &base);
        }
        let detected = Paths::detect_homes();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_DETECT_HOME");
        }
        let detected = detected.unwrap();

        let names: Vec<&str> = detected.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"gemini"), "gemini marker exists: {names:?}");
        assert!(
            names.contains(&"universal"),
            "agents marker exists: {names:?}"
        );
        assert!(
            !names.contains(&"codex"),
            "no .codex dir, so codex must not be detected: {names:?}"
        );

        // The reported gemini lobe is under the detection base (.gemini/config) and carries kinds.
        let (_, gemini_lobe) = detected.iter().find(|(n, _)| *n == "gemini").unwrap();
        assert_eq!(gemini_lobe.path, base.join(".gemini/config"));
        assert_eq!(
            gemini_lobe.kinds.as_deref(),
            Some([ItemKind::Skill].as_slice())
        );

        // Windsurf detected via .codeium/windsurf marker; lobe subdir is still .windsurf.
        assert!(
            names.contains(&"windsurf"),
            "windsurf .codeium/windsurf marker exists: {names:?}"
        );
        let (_, ws_lobe) = detected.iter().find(|(n, _)| *n == "windsurf").unwrap();
        assert_eq!(ws_lobe.path, base.join(".windsurf"));
        assert_eq!(ws_lobe.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));

        // .windsurf alone is no longer the windsurf marker; windsurf must NOT be
        // double-detected.
        assert_eq!(
            names.iter().filter(|n| **n == "windsurf").count(),
            1,
            "windsurf must appear exactly once in detected list"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // HARN-5 / windsurf: a bare `.windsurf` directory (without `.codeium/windsurf`)
    // no longer triggers windsurf detection after the marker change.
    #[test]
    fn detect_homes_windsurf_requires_codeium_marker_not_bare_windsurf_dir() {
        // spec: HARN-5
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-detect-ws-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Create .windsurf but NOT .codeium/windsurf.
        std::fs::create_dir_all(base.join(".windsurf")).unwrap();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_DETECT_HOME", &base);
        }
        let detected = Paths::detect_homes();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_DETECT_HOME");
        }
        let detected = detected.unwrap();

        let names: Vec<&str> = detected.iter().map(|(n, _)| *n).collect();
        assert!(
            !names.contains(&"windsurf"),
            "bare .windsurf dir must no longer trigger windsurf detection (marker is .codeium/windsurf): {names:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // HARN-1/HARN-2: a config lobe declaring a kinds filter flows through
    // agent_homes carrying that filter, while a bare config lobe is all-kinds.
    #[test]
    fn agent_homes_carry_config_kinds_filter() {
        // spec: HARN-1
        // spec: HARN-2
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-harn-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        std::fs::write(
            mind_home.join("config.toml"),
            "lobes = [\"/c/bare\", { path = \"/c/gem\", kinds = [\"skill\"] }]\n",
        )
        .unwrap();

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes = paths.agent_homes().unwrap();
        assert_eq!(homes.len(), 2);
        assert_eq!(homes[0].path, PathBuf::from("/c/bare"));
        assert_eq!(homes[0].kinds, None, "a bare config lobe is all-kinds");
        assert_eq!(homes[1].path, PathBuf::from("/c/gem"));
        assert_eq!(
            homes[1].kinds.as_deref(),
            Some([ItemKind::Skill].as_slice()),
            "a filtered config lobe must carry its kinds"
        );
        // And admits reflects the filter.
        assert!(homes[1].admits(ItemKind::Skill));
        assert!(!homes[1].admits(ItemKind::Rule));

        let _ = std::fs::remove_dir_all(&base);
    }

    // HARN-1/HARN-2: two config lobes naming the SAME path with DIFFERENT kinds
    // dedup to a single lobe, and the first-seen kinds win. This is the direct
    // collision case the codex+universal presets create (both resolve to
    // ~/.agents): `agent_homes` must not emit the same path twice, and must keep
    // the earlier entry's filter.
    #[test]
    fn agent_homes_dedup_collision_first_kinds_win() {
        // spec: HARN-1
        // spec: HARN-2
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-harn-dedup-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        // Same path twice: first carries [skill], second carries [agent].
        std::fs::write(
            mind_home.join("config.toml"),
            "lobes = [{ path = \"/c/dup\", kinds = [\"skill\"] }, { path = \"/c/dup\", kinds = [\"agent\"] }]\n",
        )
        .unwrap();

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes = paths.agent_homes().unwrap();
        assert_eq!(
            homes.len(),
            1,
            "same-path lobes must dedup to one entry: {homes:?}"
        );
        assert_eq!(homes[0].path, PathBuf::from("/c/dup"));
        assert_eq!(
            homes[0].kinds.as_deref(),
            Some([ItemKind::Skill].as_slice()),
            "first-seen kinds must win on a dedup collision: {homes:?}"
        );
        assert!(homes[0].admits(ItemKind::Skill));
        assert!(
            !homes[0].admits(ItemKind::Agent),
            "the losing entry's [agent] kind must not leak in"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // HARN-2: the codex and universal presets both resolve to the SAME lobe path
    // (~/.agents). `preset_lobe` must produce identical paths for the two, which
    // is what lets `agent_homes`/detect dedup collapse them. (The dedup itself is
    // covered above and in the CLI detect tests; this pins the precondition.)
    #[test]
    fn codex_and_universal_presets_share_a_path() {
        // spec: HARN-2
        // spec: HARN-4
        let codex = Paths::preset_lobe("codex").unwrap();
        let universal = Paths::preset_lobe("universal").unwrap();
        assert_eq!(
            codex.path, universal.path,
            "codex and universal must resolve to the same ~/.agents path"
        );
        assert!(codex.path.ends_with(".agents"));
        // Both are skill-only.
        assert_eq!(codex.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));
        assert_eq!(
            universal.kinds.as_deref(),
            Some([ItemKind::Skill].as_slice())
        );
    }

    // ---- resolve_lobe + Lobe::reachable tests --------------------------------

    // resolve_lobe: a Global preset with no explicit base resolves under home (~).
    #[test]
    fn resolve_lobe_global_preset_defaults_to_home() {
        // spec: STO-16
        // spec: HARN-4
        let (lobe, preset) = resolve_lobe(None, Some("gemini"), None).unwrap();
        assert!(
            lobe.path.is_absolute(),
            "path must be absolute: {:?}",
            lobe.path
        );
        assert!(
            lobe.path.ends_with(".gemini/config"),
            "gemini lobe must be at home/.gemini/config: {:?}",
            lobe.path
        );
        assert_eq!(
            lobe.kinds.as_deref(),
            Some([ItemKind::Skill].as_slice()),
            "gemini lobe must be skill-only"
        );
        let p = preset.expect("gemini must return a preset");
        assert_eq!(p.name, "gemini");
        assert_eq!(p.scope, Scope::Global);
    }

    // resolve_lobe: a Project preset with no explicit base defaults to cwd.
    #[test]
    fn resolve_lobe_project_preset_defaults_to_cwd() {
        // spec: STO-16
        // spec: HARN-4
        let (lobe, preset) = resolve_lobe(None, Some("windsurf"), None).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert!(
            lobe.path.is_absolute(),
            "path must be absolute: {:?}",
            lobe.path
        );
        assert_eq!(
            lobe.path,
            cwd.join(".windsurf"),
            "windsurf lobe with no base must resolve to cwd/.windsurf"
        );
        assert_eq!(lobe.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));
        let p = preset.expect("windsurf must return a preset");
        assert_eq!(p.scope, Scope::Project);
    }

    // resolve_lobe: an explicit base overrides the default for a Global preset.
    #[test]
    fn resolve_lobe_preset_with_explicit_base() {
        // spec: STO-16
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-rl-base-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base_dir).unwrap();

        let (lobe, preset) =
            resolve_lobe(Some(base_dir.to_str().unwrap()), Some("gemini"), None).unwrap();
        assert_eq!(
            lobe.path,
            base_dir.join(".gemini/config"),
            "explicit base must override the home default for gemini"
        );
        let p = preset.unwrap();
        assert_eq!(p.name, "gemini");

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // resolve_lobe: --subdir without a preset uses the cwd as default base.
    #[test]
    fn resolve_lobe_subdir_defaults_base_to_cwd() {
        // spec: STO-16
        let (lobe, preset) = resolve_lobe(None, None, Some(".windsurf")).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            lobe.path,
            cwd.join(".windsurf"),
            "--subdir with no base must use cwd"
        );
        assert_eq!(
            lobe.kinds.as_deref(),
            Some([ItemKind::Skill].as_slice()),
            "--subdir lobe must be skill-only"
        );
        assert!(
            preset.is_none(),
            "--subdir with no preset returns None preset"
        );
    }

    // resolve_lobe: --subdir with an explicit base joins base/subdir.
    #[test]
    fn resolve_lobe_subdir_with_explicit_base() {
        // spec: STO-16
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir = std::env::temp_dir().join(format!("mind-rl-sub-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base_dir).unwrap();

        let (lobe, _) =
            resolve_lobe(Some(base_dir.to_str().unwrap()), None, Some(".myharness")).unwrap();
        assert_eq!(lobe.path, base_dir.join(".myharness"));
        assert_eq!(lobe.kinds.as_deref(), Some([ItemKind::Skill].as_slice()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // resolve_lobe: bare path add -- just a base, no preset or subdir.
    #[test]
    fn resolve_lobe_bare_path_add() {
        // spec: STO-16
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let lobe_dir =
            std::env::temp_dir().join(format!("mind-rl-bare-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&lobe_dir).unwrap();

        let (lobe, preset) = resolve_lobe(Some(lobe_dir.to_str().unwrap()), None, None).unwrap();
        assert_eq!(
            lobe.path, lobe_dir,
            "bare path add must use base as the lobe path"
        );
        assert_eq!(lobe.kinds, None, "bare path add must be all-kinds");
        assert!(preset.is_none());

        let _ = std::fs::remove_dir_all(&lobe_dir);
    }

    // resolve_lobe: no base, no preset, no subdir -> LobeTargetRequired.
    #[test]
    fn resolve_lobe_no_args_returns_lobe_target_required() {
        // spec: STO-16
        let err = resolve_lobe(None, None, None).unwrap_err();
        assert!(
            matches!(err, MindError::LobeTargetRequired),
            "no base/preset/subdir must be LobeTargetRequired: {err:?}"
        );
    }

    // resolve_lobe: explicit base that does not exist -> LobeBaseMissing.
    #[test]
    fn resolve_lobe_missing_base_returns_lobe_base_missing() {
        // spec: STO-56
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let missing =
            std::env::temp_dir().join(format!("mind-rl-missing-{}-{n}", std::process::id()));
        // Do NOT create missing.
        let err =
            resolve_lobe(Some(missing.to_str().unwrap()), Some("windsurf"), None).unwrap_err();
        assert!(
            matches!(err, MindError::LobeBaseMissing { .. }),
            "nonexistent explicit base must be LobeBaseMissing: {err:?}"
        );
        // Also for --subdir with a missing base.
        let err2 = resolve_lobe(Some(missing.to_str().unwrap()), None, Some(".ws")).unwrap_err();
        assert!(
            matches!(err2, MindError::LobeBaseMissing { .. }),
            "--subdir with nonexistent base must be LobeBaseMissing: {err2:?}"
        );
    }

    // Lobe::reachable: true when the parent exists, false when it doesn't.
    #[test]
    fn lobe_reachable_true_when_parent_exists() {
        // spec: STO-56
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-reach-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        // base exists -> lobe at base/.windsurf: parent (base) exists -> reachable.
        let lobe = Lobe {
            path: base.join(".windsurf"),
            kinds: Some(vec![ItemKind::Skill]),
        };
        assert!(
            lobe.reachable(),
            "lobe is reachable when its parent dir exists (STO-56)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lobe_reachable_false_when_parent_missing() {
        // spec: STO-56
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let vanished = std::env::temp_dir().join(format!("mind-gone-{}-{n}", std::process::id()));
        // Do NOT create vanished.
        let lobe = Lobe {
            path: vanished.join(".windsurf"),
            kinds: Some(vec![ItemKind::Skill]),
        };
        assert!(
            !lobe.reachable(),
            "lobe is unreachable when its parent dir is missing (STO-56)"
        );
    }

    /// STO-56 explicitly carves `link_into_new_lobes` OUT of the reachability
    /// gate: it is the HARN-7/HARN-8 backfill path that runs when a lobe is
    /// newly added (e.g. `config lobes add` or a preset base such as
    /// `base/.gemini/config`), and at that moment the lobe's parent may not
    /// exist yet -- gating there would suppress the very links the backfill
    /// exists to create. This is a unit test (not an integration test in
    /// `tests/`) because `link_into_new_lobes` is a `pub fn` in `src/install.rs`
    /// (a module this shard does not own) and the crate ships no `[lib]`
    /// target, so an integration test in `tests/` can only drive the compiled
    /// `mind` binary as a subprocess -- it has no way to call a Rust function
    /// directly. Any test in this single-binary crate can call another
    /// module's `pub`/`pub(crate)` items via `crate::...` without editing that
    /// module, so this unit test (here, in `src/paths.rs`, which owns
    /// `Lobe::reachable`) is the reachable option that requires no edit to
    /// `install.rs`.
    #[test]
    fn link_into_new_lobes_links_into_a_lobe_with_a_missing_parent() {
        // spec: STO-56
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-link-new-lobes-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        let mind_home = base.join("mind");
        mkdir_p(&mind_home).unwrap();
        // A real store copy for the item to be symlinked from.
        let store_rel = "store/skill/greet";
        let store_abs = mind_home.join(store_rel);
        std::fs::create_dir_all(&store_abs).unwrap();
        std::fs::write(store_abs.join("SKILL.md"), "---\ndescription: greet\n---\n").unwrap();

        let paths = Paths {
            mind_home: mind_home.clone(),
            claude_home: base.join("claude-unused"),
        };

        let item = crate::manifest::InstalledItem {
            kind: ItemKind::Skill,
            name: "greet".to_string(),
            bare_name: "greet".to_string(),
            source: "test-source".to_string(),
            commit: "deadbeef".to_string(),
            hash: "abc123".to_string(),
            store: store_rel.to_string(),
            links: vec![], // empty: link_rel falls back to default_link_rel
            description: None,
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
        };

        // A lobe whose PARENT directory does not exist -- freshly added, not yet
        // materialized on disk (e.g. a project that has not run `mkdir .windsurf`
        // yet, or a preset base backfill).
        let missing_parent = base.join("not-yet-created");
        let lobe = Lobe::all_kinds(missing_parent.join(".windsurf"));
        assert!(
            !lobe.reachable(),
            "sanity: the lobe's parent must genuinely be missing for this test"
        );

        let (created, failed) = crate::install::link_into_new_lobes(&paths, &item, &[lobe], false);

        assert!(
            failed.is_empty(),
            "link_into_new_lobes must not fail when the lobe's parent is missing \
             (STO-56 explicitly excludes this call site from the reachability gate): {failed:?}"
        );
        assert_eq!(
            created.len(),
            1,
            "exactly one link must be created: {created:?}"
        );
        let expected = missing_parent.join(".windsurf/skills/greet");
        assert_eq!(
            created[0], expected,
            "created link must be at the expected path"
        );
        assert!(
            expected.symlink_metadata().is_ok(),
            "the symlink must actually exist on disk at {expected:?} -- a regression that \
             added the STO-56 gate to link_into_new_lobes would silently create nothing here \
             while this whole test suite otherwise stays green (HARN-7 preset backfill)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lobe_reachable_global_home_always_reachable() {
        // spec: STO-56
        // A global home like ~/.claude always has a parent (~) that exists.
        let home = dirs::home_dir().expect("home dir must exist in test env");
        let claude = home.join(".claude");
        let lobe = Lobe::all_kinds(claude);
        assert!(
            lobe.reachable(),
            "global home lobe parent (~) always exists (STO-56)"
        );
    }

    // resolve_lobe: windsurf scope is Project; gemini/codex/universal are Global.
    #[test]
    fn preset_scope_assignments() {
        // spec: HARN-4
        assert_eq!(lookup_preset("gemini").unwrap().scope, Scope::Global);
        assert_eq!(lookup_preset("codex").unwrap().scope, Scope::Global);
        assert_eq!(lookup_preset("universal").unwrap().scope, Scope::Global);
        assert_eq!(lookup_preset("windsurf").unwrap().scope, Scope::Project);
    }
}
