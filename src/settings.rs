//! Every read of mind's settings: the process environment ([`Env`]), the user
//! config (`config.toml`) and the managed policy file ([`Settings`]).
//!
//! Nothing outside this module calls `std::env::var*`, `dirs::home_dir`,
//! `Config` deserialization, or the policy-file lookup. Each setting has one
//! method here, so its name, precedence, and blank-value handling live in one
//! place. Reads are live (no caching): the TUI runs commands in-process and
//! tests set variables between calls, so a value read once at startup would
//! go stale.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::Config;
use crate::error::{MindError, Result};
use crate::paths::Paths;
use crate::policy::Policy;

/// `MIND_HOME`: overrides the default mind home.
pub const MIND_HOME: &str = "MIND_HOME";
/// `MIND_DEFAULT_LOBE`: the default lobe; outranks [`CLAUDE_HOME`] (CLI-170).
pub const MIND_DEFAULT_LOBE: &str = "MIND_DEFAULT_LOBE";
/// `CLAUDE_HOME`: the legacy default-lobe override.
pub const CLAUDE_HOME: &str = "CLAUDE_HOME";
/// `MIND_HOMES`: extra mind homes, a path list (HOME-2).
pub const MIND_HOMES: &str = "MIND_HOMES";
/// `MIND_AGENT_HOMES`: the lobes, a `:`-separated list overriding config (HARN-2).
pub const MIND_AGENT_HOMES: &str = "MIND_AGENT_HOMES";
/// `MIND_DETECT_HOME`: the base `config lobes detect` scans (HARN-5).
pub const MIND_DETECT_HOME: &str = "MIND_DETECT_HOME";
/// `MIND_POLICY_FILE`: the policy file used when no system file exists (POL-2).
pub const MIND_POLICY_FILE: &str = "MIND_POLICY_FILE";
/// `MIND_ABSORB_TO`: the default `absorb` destination (ABS-2).
pub const MIND_ABSORB_TO: &str = "MIND_ABSORB_TO";
/// `MIND_MAX_METADATA_SIZE`: the metadata cap, between the flag and the config
/// key in precedence (DSC-104).
pub const MIND_MAX_METADATA_SIZE: &str = "MIND_MAX_METADATA_SIZE";
/// `MIND_MAX_PARALLEL_FETCHES`: the most source fetches run at once.
pub const MIND_MAX_PARALLEL_FETCHES: &str = "MIND_MAX_PARALLEL_FETCHES";
/// `MIND_HTTP_TIMEOUT_SECS`: the `evolve` HTTP timeout.
pub const MIND_HTTP_TIMEOUT_SECS: &str = "MIND_HTTP_TIMEOUT_SECS";
/// `MIND_TTY`: overrides whether stdin is a terminal (HOOK-109).
pub const MIND_TTY: &str = "MIND_TTY";
/// `MIND_IN_HOOK`: set to `1` on every hook `mind` runs (HOOK-144).
pub const MIND_IN_HOOK: &str = "MIND_IN_HOOK";
/// `MIND_PROGRESS`: the progress-spinner setting.
pub const MIND_PROGRESS: &str = "MIND_PROGRESS";
/// `MIND_TEST_CRASH_AT`: debug-build fault injection (LIFE-63).
pub const MIND_TEST_CRASH_AT: &str = "MIND_TEST_CRASH_AT";
/// The token variables the updater reads, in its precedence order (STO-57:
/// `gh`'s order, first set and non-empty wins). Mirrors the updater crate's
/// `pub(crate)` list.
pub const GITHUB_TOKEN_VARS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN"];
/// The locale variables, in POSIX precedence order.
const LOCALE_VARS: &[&str] = &["LC_ALL", "LC_CTYPE", "LANG"];

/// The process environment, one method per variable mind reads.
///
/// Zero-sized, so code that runs before [`Paths`] exists (output setup, the
/// lock, hooks, git) reads it as `Env`; code holding a `Paths` reaches the same
/// reader through [`Settings::env`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Env;

/// A variable's value, `None` when unset or set to only whitespace.
fn non_blank(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

impl Env {
    /// `$MIND_HOME`.
    pub fn mind_home(self) -> Option<PathBuf> {
        std::env::var_os(MIND_HOME).map(PathBuf::from)
    }

    /// The default-lobe override: `$MIND_DEFAULT_LOBE`, else `$CLAUDE_HOME`
    /// (CLI-170).
    pub fn default_lobe(self) -> Option<OsString> {
        std::env::var_os(MIND_DEFAULT_LOBE).or_else(|| std::env::var_os(CLAUDE_HOME))
    }

    /// `$MIND_HOMES`, the raw path list (HOME-2).
    pub fn mind_homes(self) -> Option<OsString> {
        std::env::var_os(MIND_HOMES)
    }

    /// `$MIND_AGENT_HOMES`, the raw `:`-separated lobe list (HARN-2).
    pub fn agent_homes(self) -> Option<OsString> {
        std::env::var_os(MIND_AGENT_HOMES)
    }

    /// `$MIND_DETECT_HOME` (HARN-5).
    pub fn detect_home(self) -> Option<PathBuf> {
        std::env::var_os(MIND_DETECT_HOME).map(PathBuf::from)
    }

    /// `$MIND_POLICY_FILE` (POL-2).
    pub fn policy_file(self) -> Option<PathBuf> {
        std::env::var_os(MIND_POLICY_FILE).map(PathBuf::from)
    }

    /// `%PROGRAMDATA%`, the base of the Windows system policy path (POL-1).
    #[cfg(target_os = "windows")]
    pub fn program_data(self) -> Option<PathBuf> {
        std::env::var_os("PROGRAMDATA").map(PathBuf::from)
    }

    /// `$MIND_ABSORB_TO` (ABS-2).
    pub fn absorb_to(self) -> Option<OsString> {
        std::env::var_os(MIND_ABSORB_TO)
    }

    /// `$MIND_MAX_METADATA_SIZE`, raw: startup tells unset, blank, and
    /// undecodable apart (DSC-111).
    pub fn max_metadata_size(self) -> Option<OsString> {
        std::env::var_os(MIND_MAX_METADATA_SIZE)
    }

    /// `$MIND_MAX_PARALLEL_FETCHES`, unparsed.
    pub fn max_parallel_fetches(self) -> Option<String> {
        std::env::var(MIND_MAX_PARALLEL_FETCHES).ok()
    }

    /// `$MIND_HTTP_TIMEOUT_SECS` as a number, `None` when unset or not one.
    pub fn http_timeout_secs(self) -> Option<u64> {
        std::env::var(MIND_HTTP_TIMEOUT_SECS)
            .ok()
            .and_then(|v| v.parse().ok())
    }

    /// Whether a GitHub token variable is set to something other than
    /// whitespace (STO-57's emptiness rule).
    pub fn github_token_set(self) -> bool {
        GITHUB_TOKEN_VARS.iter().any(|v| non_blank(v).is_some())
    }

    /// `$MIND_TTY`, raw (HOOK-109).
    pub fn tty(self) -> Option<OsString> {
        std::env::var_os(MIND_TTY)
    }

    /// Whether this process is a hook `mind` ran (`$MIND_IN_HOOK=1`, HOOK-144).
    pub fn in_hook(self) -> bool {
        std::env::var_os(MIND_IN_HOOK).is_some_and(|v| v == "1")
    }

    /// `$MIND_PROGRESS`, unparsed.
    pub fn progress(self) -> Option<String> {
        std::env::var(MIND_PROGRESS).ok()
    }

    /// Whether `$NO_COLOR` is set (any value).
    pub fn no_color(self) -> bool {
        std::env::var_os("NO_COLOR").is_some()
    }

    /// `$COLUMNS` as a number, `None` when unset or not one.
    pub fn columns(self) -> Option<usize> {
        std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
    }

    /// Whether `$TERM` is `dumb`.
    pub fn term_dumb(self) -> bool {
        std::env::var("TERM").is_ok_and(|t| t == "dumb")
    }

    /// `$CI`, unparsed.
    pub fn ci(self) -> Option<String> {
        std::env::var("CI").ok()
    }

    /// Whether the locale is UTF-8: the first of `LC_ALL`, `LC_CTYPE`, `LANG`
    /// that is set and non-empty names a UTF-8 charset. `false` when none is
    /// set (the conservative ASCII default).
    pub fn utf8_locale(self) -> bool {
        for var in LOCALE_VARS {
            if let Some(val) = std::env::var_os(var) {
                let lower = val.to_string_lossy().to_lowercase();
                if lower.is_empty() {
                    continue;
                }
                return lower.contains("utf-8") || lower.contains("utf8");
            }
        }
        false
    }

    /// `$GIT_SSH_COMMAND`, `None` when unset or blank.
    pub fn git_ssh_command(self) -> Option<String> {
        non_blank("GIT_SSH_COMMAND")
    }

    /// `$GIT_SSH`, `None` when unset or blank.
    pub fn git_ssh(self) -> Option<String> {
        non_blank("GIT_SSH")
    }

    /// `$GIT_SSH_VARIANT`, `None` when unset or blank.
    pub fn git_ssh_variant(self) -> Option<String> {
        non_blank("GIT_SSH_VARIANT")
    }

    /// `$GIT_ALLOW_PROTOCOL`, `None` only when unset: git reads an empty value
    /// as "allow no protocol", so a blank one is kept (POL-70).
    pub fn git_allow_protocol(self) -> Option<String> {
        std::env::var_os("GIT_ALLOW_PROTOCOL").map(|v| v.to_string_lossy().into_owned())
    }

    /// `$GIT_CONFIG_COUNT` as a number, `0` when unset or not a number: the
    /// count of `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` pairs already set.
    pub fn git_config_count(self) -> usize {
        std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }

    /// The user's home directory (`$HOME`, or the platform lookup).
    pub fn home_dir(self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    /// `path` with a leading `~` or `~/` expanded to [`Self::home_dir`]; any
    /// other path, or any path when there is no home, passes through.
    pub fn expand_tilde(self, path: &str) -> PathBuf {
        if path == "~" {
            return self.home_dir().unwrap_or_else(|| PathBuf::from(path));
        }
        if let Some(rest) = path.strip_prefix("~/")
            && let Some(home) = self.home_dir()
        {
            return home.join(rest);
        }
        PathBuf::from(path)
    }

    /// `$MIND_TEST_CRASH_AT` as a number (LIFE-63). Debug builds only.
    #[cfg(debug_assertions)]
    pub fn crash_at(self) -> Option<u64> {
        std::env::var(MIND_TEST_CRASH_AT).ok()?.parse().ok()
    }
}

/// The file-backed settings of one set of [`Paths`]: the user config and the
/// managed policy, plus the [`Env`] they are resolved against.
#[derive(Debug, Clone, Copy)]
pub struct Settings<'a> {
    paths: &'a Paths,
}

impl<'a> Settings<'a> {
    pub(crate) fn new(paths: &'a Paths) -> Self {
        Self { paths }
    }

    /// The process environment.
    pub fn env(self) -> Env {
        Env
    }

    /// `config.toml` from the mind home; defaults when the file is absent.
    pub fn config(self) -> Result<Config> {
        let file = self.paths.config_file();
        match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).map_err(|e| MindError::ConfigToml {
                path: file.clone(),
                msg: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    /// `config.toml`'s raw text, `None` when it cannot be read. For reporting
    /// on a config [`Self::config`] refused to parse.
    pub fn config_text(self) -> Option<String> {
        std::fs::read_to_string(self.paths.config_file()).ok()
    }

    /// Write `cfg` to `config.toml`, creating the mind home if needed.
    pub fn save_config(self, cfg: &Config) -> Result<()> {
        let home = &self.paths.mind_home;
        std::fs::create_dir_all(home).map_err(|e| MindError::io(home, e))?;
        let file = self.paths.config_file();
        let text = toml::to_string(cfg).map_err(|e| MindError::TomlWrite {
            path: file.clone(),
            source: e,
        })?;
        Paths::atomic_write(&file, text.as_bytes())
    }

    /// The managed policy, or `None` when none is configured (POL-4 inert).
    /// The system path wins; `$MIND_POLICY_FILE` is read only when no system
    /// file exists (POL-1, POL-2). A parse error, unknown key, or failed
    /// validation is `Err` (POL-5 fail closed).
    pub fn policy(self) -> Result<Option<Policy>> {
        match self.policy_path() {
            // spec: POL-65 -- permission check only for the real system path.
            Some(path) => {
                let is_system = path == crate::policy::system_path();
                crate::policy::load_at(&path, is_system).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Whether the policy in effect forbids cleartext transports (POL-70), read
    /// without printing the POL-64 permission warnings. A policy that will not
    /// load counts as forbidding them (POL-5 fail closed).
    pub fn forbids_insecure_transport(self) -> bool {
        self.policy_path().is_some_and(|path| {
            crate::policy::load_file(&path).map_or(true, |p| !p.allow_insecure_transport())
        })
    }

    /// The policy file in effect, if any: the system path when it exists, else
    /// `$MIND_POLICY_FILE` when set and the file exists (POL-1, POL-2).
    pub fn policy_path(self) -> Option<PathBuf> {
        crate::policy::locate_existing(&crate::policy::system_path(), Env.policy_file().as_deref())
    }
}

impl Paths {
    /// The settings resolved for these paths.
    pub fn settings(&self) -> Settings<'_> {
        Settings::new(self)
    }
}
