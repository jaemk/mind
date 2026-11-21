//! `mind config show`.

use super::*;

/// `mind config show` — print the config file location and its key/value pairs.
pub fn config_show(paths: &Paths) -> Result<()> {
    let out = crate::render::ctx();
    paths.ensure_config()?;
    let file = paths.config_file();
    let cfg = paths.settings().config()?;
    // spec: CLI-241 -- the effective cap, not the config key: the flag and the
    // environment outrank it (DSC-104), so reporting the key alone would name a
    // value that is not the one in force.
    let limit = crate::error::metadata_size_limit();
    // spec: CLI-241 HOOK-165 HOOK-170 -- likewise the hook logging in force:
    // `--hook-log` and `--hook-log-keep` outrank the config keys. When they
    // agree with the keys, the keys are shown as written.
    let config_keep = cfg.hook_log_keep.unwrap_or_default();
    let flag_logging = crate::hook_log::logging().filter(|(logs, keep, owned)| {
        *logs != crate::hook_log::config_logs(cfg.hook_log.as_ref(), owned, &paths.mind_home)
            || *keep != config_keep
    });
    let hook_log = match &flag_logging {
        Some((None, _, _)) => crate::config::HookLogSetting::Enabled(false),
        Some((Some(dir), _, owned)) if dir == owned => crate::config::HookLogSetting::Enabled(true),
        Some((Some(dir), _, _)) => crate::config::HookLogSetting::Dir(dir.display().to_string()),
        // Unset is off, reported as `false`.
        None => cfg
            .hook_log
            .clone()
            .unwrap_or(crate::config::HookLogSetting::Enabled(false)),
    };
    let keep = flag_logging
        .as_ref()
        .map_or(config_keep, |(_, keep, _)| *keep);
    let extra_homes = paths.extra_homes();
    if out.json {
        return print_json(&serde_json::json!({
            "config_file": file.display().to_string(),
            "lobes": cfg.lobes,
            "default_lobe": paths.claude_home.display().to_string(),
            "ssh": cfg.ssh,
            "max_metadata_size": crate::error::format_metadata_size(limit),
            // spec: CLI-242 -- every integer mind produces in JSON fits an
            // i64, so a consumer that parses numbers as signed 64-bit does not
            // overflow on the unlimited cap (held as u64::MAX).
            "max_metadata_size_bytes": limit.min(i64::MAX as u64),
            // spec: HOOK-164 HOOK-165
            "hook_history": cfg.hook_history.unwrap_or(crate::hook_log::DEFAULT_HISTORY),
            "hook_log": hook_log,
            // spec: HOOK-170
            "hook_log_keep": keep,
            // spec: HOME-2 -- the extra homes in force (`MIND_HOMES` and the
            // config key), resolved.
            "mind_homes": extra_homes
                .iter()
                .map(|h| h.display().to_string())
                .collect::<Vec<_>>(),
            // spec: HOME-17 -- the homes mind recorded for sources, and
            // whether the user's own list restricts homes and lobes.
            "source_homes": cfg.source_homes,
            "homes_restricted": paths.homes_restricted(),
            // spec: LIFE-61 -- the extra-home source lobes recovery trusts.
            "known_lobes": cfg.known_lobes,
        }));
    }
    println!("{} config file: {}", out.bullet(), file.display());
    if cfg.lobes.is_empty() {
        println!(
            "  {} lobes = []  (default: {})",
            out.dim("·"),
            paths.claude_home.display()
        );
    } else {
        let rendered: Vec<String> = cfg.lobes.iter().map(format_lobe).collect();
        println!("  {} lobes = {}", out.dim("·"), rendered.join(", "));
    }
    println!(
        "  {} ssh = {}  (prefer SSH for melded remotes)",
        out.dim("·"),
        cfg.ssh
    );
    println!(
        "  {} max-metadata-size = {}  (cap on every metadata file read from a source)",
        out.dim("·"),
        crate::error::format_metadata_size(limit)
    );
    // spec: HOOK-164 HOOK-165
    println!(
        "  {} hook-history = {}  (hook results kept for `mind hooks history`)",
        out.dim("·"),
        cfg.hook_history.unwrap_or(crate::hook_log::DEFAULT_HISTORY)
    );
    let hook_log = match &hook_log {
        crate::config::HookLogSetting::Enabled(b) => b.to_string(),
        crate::config::HookLogSetting::Dir(d) => format!("{d:?}"),
    };
    println!(
        "  {} hook-log = {hook_log}  (save each hook's output to a log file)",
        out.dim("·")
    );
    // spec: HOOK-170
    let keep = match keep {
        crate::hook_log::LogKeep::All => "all",
        crate::hook_log::LogKeep::Failed => "failed",
    };
    println!(
        "  {} hook-log-keep = {keep}  (which hook logs to keep: all, or failed only)",
        out.dim("·")
    );
    // spec: HOME-2
    let quoted = |hs: &[std::path::PathBuf]| -> String {
        hs.iter()
            .map(|h| format!("{:?}", h.display().to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let user_homes = paths.user_homes();
    println!(
        "  {} mind-homes = [{}]  (extra mind homes, with MIND_HOMES; {} is always allowed)",
        out.dim("·"),
        quoted(&user_homes),
        paths.mind_home.display()
    );
    // spec: HOME-17
    if !cfg.source_homes.is_empty() {
        let note = if user_homes.is_empty() {
            "homes sources were melded into"
        } else {
            "not read while mind-homes or MIND_HOMES is set"
        };
        let recorded: Vec<std::path::PathBuf> = cfg
            .source_homes
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        println!(
            "  {} source-homes = [{}]  ({note})",
            out.dim("·"),
            quoted(&recorded)
        );
    }
    // spec: LIFE-61
    if !cfg.known_lobes.is_empty() {
        let recorded: Vec<std::path::PathBuf> = cfg
            .known_lobes
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        println!(
            "  {} known-lobes = [{}]  (lobes of sources in other homes that mind linked into)",
            out.dim("·"),
            quoted(&recorded)
        );
    }
    // spec: CLI-241 DSC-104 -- the value printed is the effective one, which
    // the flag and the environment can outrank, so say where it came from when
    // it is not what the config key says. Without this the line reads as a
    // report of `config.toml` and quietly is not one, the same trap the
    // MIND_AGENT_HOMES note below exists for.
    if let Some(note) = metadata_cap_origin(crate::config::metadata_limit_origin()) {
        println!("note: {note}");
    }
    if flag_logging.is_some() {
        println!(
            "note: hook-log and hook-log-keep are set by --hook-log / --hook-log-keep for this run"
        );
    }
    if let Some(env) = paths.settings().env().agent_homes() {
        println!(
            "note: MIND_AGENT_HOMES is set and overrides lobes: {}",
            env.to_string_lossy()
        );
    }
    Ok(())
}

/// Where `config show`'s effective metadata cap came from, when that is not the
/// `max-metadata-size` config key it is printed beside (DSC-104).
///
/// Pure in the origin `main::run` recorded at startup, so the credit goes to
/// the origin that actually won rather than to whichever one happens to parse
/// to the same number: the flag is named whenever it was given, even when its
/// value equals the config key or the default. `None` for the config key, the
/// built-in default, or no recorded origin -- the printed line already tells
/// the whole story there and a note would be noise.
fn metadata_cap_origin(origin: Option<&crate::config::MetadataLimitOrigin>) -> Option<String> {
    use crate::config::MetadataLimitOrigin;
    match origin? {
        MetadataLimitOrigin::Flag(raw) => Some(format!(
            "the max-metadata-size shown is this run's --max-metadata-size ({}), which \
             overrides the environment and the config key",
            crate::sanitize::strip_ansi(raw)
        )),
        MetadataLimitOrigin::Env(raw) => Some(format!(
            "{} is set and overrides the max-metadata-size config key: {}",
            crate::settings::MIND_MAX_METADATA_SIZE,
            crate::sanitize::strip_ansi(raw)
        )),
        MetadataLimitOrigin::Config(_) | MetadataLimitOrigin::Default => None,
    }
}

/// Render a lobe entry for display: the path, plus its `kinds` filter in brackets
/// when present (HARN-1). A no-kinds lobe shows just the path (it admits all).
pub(super) fn format_lobe(entry: &crate::config::LobeEntry) -> String {
    match entry.kinds() {
        None => entry.path().to_string(),
        Some(kinds) => {
            let names: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
            format!("{} [{}]", entry.path(), names.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `config show` prints the EFFECTIVE metadata cap, which the flag and the
    /// environment outrank (DSC-104), so it names the origin whenever that is
    /// not the config key beside it -- and stays silent when the printed value
    /// needs no explanation, so an ordinary `config show` gains no noise.
    /// Pure in the recorded origin: no environment read, no env lock.
    // spec: CLI-241 DSC-104
    #[test]
    fn config_show_names_where_a_non_config_metadata_cap_came_from() {
        use crate::config::MetadataLimitOrigin;
        // Nothing recorded, the default, or the config key: no note.
        assert_eq!(metadata_cap_origin(None), None);
        assert_eq!(
            metadata_cap_origin(Some(&MetadataLimitOrigin::Default)),
            None
        );
        assert_eq!(
            metadata_cap_origin(Some(&MetadataLimitOrigin::Config("2MiB".into()))),
            None
        );
        // The flag is credited with its raw value, even when that value equals
        // the default (the flag still won).
        let note = metadata_cap_origin(Some(&MetadataLimitOrigin::Flag("8MiB".into())))
            .expect("the flag must be named");
        assert!(
            note.contains("--max-metadata-size (8MiB)")
                && note.contains("overrides the environment and the config key"),
            "{note}"
        );
        // The environment is named by its constant, with its raw value.
        let note = metadata_cap_origin(Some(&MetadataLimitOrigin::Env("4KiB".into())))
            .expect("the environment must be named");
        assert!(
            note.contains(crate::settings::MIND_MAX_METADATA_SIZE) && note.contains("4KiB"),
            "{note}"
        );
    }
}
