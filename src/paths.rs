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
//! A lobe is the parent of `skills/` / `agents/` / `rules/`; the default is
//! `~/.claude`, but a lobe may be any harness home (Gemini, Codex, Antigravity)
//! because the skill/agent layouts double as the cross-tool conventions
//! (spec/harness-lobes.md). A lobe may carry a `kinds` filter (HARN-1): only
//! items of a listed kind link into it. The [`PRESETS`] table maps a harness name
//! to its lobe path and kinds (HARN-4), and [`detect_homes`] reports which preset
//! dirs exist under the detection base (HARN-5), consulting `MIND_DETECT_HOME`
//! (else the home dir) so detection stays hermetic without mutating process HOME.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::policy::Policy;

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
}

/// A known harness preset (HARN-4): the lobe path (relative to the detection
/// base / home) and the kinds it admits. `marker_rel` is the on-disk signal
/// [`detect_homes`] checks to decide the harness is installed.
pub struct Preset {
    /// The preset name used on the CLI (`--preset <name>`).
    pub name: &'static str,
    /// The lobe parent directory, relative to home (e.g. `.gemini`).
    pub rel_path: &'static str,
    /// The kinds this preset's lobe admits.
    pub kinds: &'static [ItemKind],
    /// The directory whose presence signals this harness is installed, relative
    /// to the detection base (e.g. `.gemini` for Gemini, `.codex` for Codex).
    pub marker_rel: &'static str,
}

/// The harness presets (HARN-4). Detection signals (HARN-5):
/// - `gemini`: `~/.gemini` exists (Gemini CLI / Antigravity shared home; lobe is `~/.gemini/config`).
/// - `codex`: `~/.codex` exists (Codex CLI's home; it reads `~/.agents`).
/// - `universal`: `~/.agents` exists (the vendor-neutral alias dir itself).
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "gemini",
        rel_path: ".gemini/config",
        kinds: &[ItemKind::Skill],
        marker_rel: ".gemini",
    },
    Preset {
        name: "codex",
        rel_path: ".agents",
        kinds: &[ItemKind::Skill],
        marker_rel: ".codex",
    },
    Preset {
        name: "universal",
        rel_path: ".agents",
        kinds: &[ItemKind::Skill],
        marker_rel: ".agents",
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
            ItemKind::Agent | ItemKind::Rule => Some(format!("{dir}/{name}.md")),
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

    /// The lobe a `--preset <name>` resolves to (HARN-4): the preset's parent
    /// path (with `~` expanded to absolute, STO-16) and its kinds filter. Errors
    /// with [`MindError::UnknownPreset`] on a bad name.
    pub fn preset_lobe(name: &str) -> Result<Lobe> {
        let preset = lookup_preset(name)?;
        Ok(Lobe {
            path: absolute_home(&format!("~/{}", preset.rel_path))?,
            kinds: Some(preset.kinds.to_vec()),
        })
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

        // Write to temp; clean up on error.
        let write_result =
            std::fs::write(&tmp_path, bytes).map_err(|e| MindError::io(&tmp_path, e));
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

/// Deduplicate a `Vec<Lobe>` by path, preserving first-seen order. When the same
/// path appears twice, the first-seen lobe (and its kinds) wins.
fn dedup_lobes(lobes: Vec<Lobe>) -> Vec<Lobe> {
    let mut seen = std::collections::HashSet::new();
    lobes
        .into_iter()
        .filter(|l| seen.insert(l.path.clone()))
        .collect()
}

/// `mkdir -p` that tags failures with the offending path.
pub fn mkdir_p(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| MindError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Serialize all tests that touch process-global env vars (`MIND_POLICY_FILE`,
    /// `MIND_AGENT_HOMES`, `MIND_HOME`, `CLAUDE_HOME`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    // name errors with UnknownPreset.
    #[test]
    fn preset_lookup_and_resolution() {
        // spec: HARN-4
        let gemini = lookup_preset("gemini").unwrap();
        assert_eq!(gemini.rel_path, ".gemini/config");
        assert_eq!(gemini.kinds, &[ItemKind::Skill]);

        let codex = lookup_preset("codex").unwrap();
        assert_eq!(codex.rel_path, ".agents");
        assert_eq!(codex.kinds, &[ItemKind::Skill]);

        assert_eq!(lookup_preset("universal").unwrap().rel_path, ".agents");

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
    #[test]
    fn detect_homes_reports_existing_marker_dirs() {
        // spec: HARN-5
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-detect-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Create .gemini and .agents but NOT .codex/.gemini/config.
        std::fs::create_dir_all(base.join(".gemini")).unwrap();
        std::fs::create_dir_all(base.join(".agents")).unwrap();

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
}
