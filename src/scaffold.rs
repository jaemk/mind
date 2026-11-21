//! Pure helpers for `init-source` scaffolding (INIT-10, INIT-11, INIT-12).
//!
//! Three public functions cover the three new behaviors; no filesystem I/O here.

// The same scaffold text written by INIT-3; kept here so `patch_source_meta`
// can start from it when no mind.toml exists yet. `{version}` is filled in by
// [`scaffold`] with the running release (INIT-13).
// The `[source]` table accepts both `namespace` and `prefix` (DSC-82 alias).
// On write, `namespace` is the canonical key (DSC-82); any active `prefix =`
// line is stripped so a patched file never carries both keys.
const SCAFFOLD: &str = concat!(
    "[source]\n",
    "description = \"\"   # what this source offers\n",
    "min-mind-version = \"{version}\"   # older mind versions warn before melding\n",
    "# namespace = \"prefix\"   # namespace items as prefix:<name>\n",
    "\n",
    "# Declare hooks that run when a consumer melds, upgrades, or unmelds this\n",
    "# source. Remove the leading `# ` to enable a hook.\n",
    "#\n",
    "# An install hook re-runs whenever the source advances, so write it to be\n",
    "# idempotent (safe to run again on a host it already set up). If a step\n",
    "# cannot be made idempotent, declare an update hook: it runs at upgrade\n",
    "# INSTEAD of the install hooks.\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make install\"         # shell command to run\n",
    "# name = \"Build\"               # optional label shown in the prompt\n",
    "# event = \"install\"             # \"install\" (default), \"update\", \"uninstall\", or \"items\"\n",
    "# optional = false              # false = required (default). optional only lets the\n",
    "#                               # user decline running it; a failure always aborts.\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make upgrade\"          # runs at upgrade in place of the install hooks\n",
    "# name = \"Upgrade\"\n",
    "# event = \"update\"\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make clean\"            # cleanup hook run at unmeld time\n",
    "# name = \"Cleanup\"\n",
    "# event = \"uninstall\"\n",
    "# optional = true               # the user may decline this step (its failure still aborts)\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make registry\"         # runs once per command that changes the installed items\n",
    "# name = \"Registry\"\n",
    "# event = \"items\"               # $MIND_ITEMS_FILE holds the full installed set: make the\n",
    "#                               # host match it, so a re-run with the same set changes nothing\n",
);

/// INIT-3 / INIT-13: the scaffold `init-source` writes for a new `mind.toml`,
/// with `min-mind-version` set to the running release.
pub(crate) fn scaffold() -> String {
    SCAFFOLD.replace("{version}", crate::mindfile::running_release_version())
}

/// INIT-11: resolve the effective plugin name from the three-level priority.
///
/// `namespace_flag` (highest) > `mindfile_prefix` > `dir_basename`.
pub fn plugin_name(
    dir_basename: &str,
    mindfile_prefix: Option<&str>,
    namespace_flag: Option<&str>,
) -> String {
    if let Some(ns) = namespace_flag {
        return ns.to_string();
    }
    if let Some(p) = mindfile_prefix {
        return p.to_string();
    }
    dir_basename.to_string()
}

/// INIT-10: produce the `.claude-plugin/marketplace.json` content.
///
/// When `skills` is `None` the `"skills"` key is omitted entirely.
/// When `skills` is `Some(&[])` the key is present with an empty array.
pub fn render_marketplace_json(name: &str, description: &str, skills: Option<&[String]>) -> String {
    let mut plugin = serde_json::Map::new();
    plugin.insert("name".into(), serde_json::Value::String(name.to_string()));
    plugin.insert("source".into(), serde_json::Value::String(".".to_string()));
    plugin.insert(
        "description".into(),
        serde_json::Value::String(description.to_string()),
    );
    if let Some(paths) = skills {
        plugin.insert(
            "skills".into(),
            serde_json::Value::Array(
                paths
                    .iter()
                    .map(|p| serde_json::Value::String(p.clone()))
                    .collect(),
            ),
        );
    }

    let manifest = serde_json::json!({
        "name": name,
        "plugins": [serde_json::Value::Object(plugin)],
    });

    let mut out =
        serde_json::to_string_pretty(&manifest).expect("JSON serialization is infallible");
    out.push('\n');
    out
}

/// INIT-12: text-patch the `[source]` section of a `mind.toml`.
///
/// When `existing_content` is `None`, starts from the built-in scaffold.
/// Each requested key is inserted after the `[source]` header if absent, or
/// the existing active line is replaced in place. Comments are preserved.
pub fn patch_source_meta(
    existing_content: Option<&str>,
    flat_skills: bool,
    namespace: Option<&str>,
) -> String {
    let mut result = existing_content.map_or_else(scaffold, str::to_string);

    if flat_skills {
        result = set_key(&result, "flat-skills", "true");
    }
    if let Some(ns) = namespace {
        let value = toml_string(ns);
        // spec: INIT-11 / DSC-82 -- write `namespace =` as the active key.
        // `set_key("namespace", ...)` replaces any existing `namespace =` line
        // or inserts a new one after the `[source]` header.
        result = set_key(&result, "namespace", &value);
        // spec: DSC-82 -- strip any active `prefix =` line that may remain
        // (the deprecated alias key). This prevents a file from ending up with
        // both `namespace =` and `prefix =`.
        // Only `[source]` is touched: a nested entry's keys are its own.
        let mut in_source = false;
        result = result
            .lines()
            .filter(|l| {
                if let Some(header) = table_header(l) {
                    in_source = is_source_header(header);
                }
                !(in_source && is_active_key(l, "prefix"))
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }

    result
}

/// The default release-tag template `init-source` declares (VER-131).
pub const RELEASE_TAGS: &str = "v{version}";

/// VER-131: declare `release-tags = "v{version}"` in a generated `mind.toml`.
pub fn with_release_tags(content: &str) -> String {
    set_key(content, "release-tags", &toml_string(RELEASE_TAGS))
}

/// The table header a line opens (`[source]`, `[[discover.sources]]`), with
/// any trailing comment dropped; `None` for any other line.
fn table_header(line: &str) -> Option<&str> {
    let t = line.trim();
    t.starts_with('[')
        .then(|| t.split_once('#').map_or(t, |(h, _)| h).trim_end())
}

/// Whether a [`table_header`] opens the `[source]` table, in any spelling TOML
/// allows for it: `[source]`, `[ source ]`, `["source"]`, `['source']`.
/// Missing one would append a second `[source]` table, which TOML rejects.
fn is_source_header(header: &str) -> bool {
    header
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .filter(|h| !h.starts_with('['))
        .is_some_and(|h| matches!(h.trim(), "source" | "\"source\"" | "'source'"))
}

/// Whether `line` is an active (uncommented) `<key> = ...` assignment.
fn is_active_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// Replace the active `<key> = ...` line in the `[source]` table, or insert it
/// after the `[source]` header. Commented lines, and lines in any other table
/// (a `[[discover.sources]]` entry has its own `namespace`), are left
/// untouched.
///
/// When no `[source]` section exists, appends one at the end of the file
/// before inserting the key.
///
/// Always emits a trailing newline; normalizes content that lacks one.
fn set_key(content: &str, key: &str, value: &str) -> String {
    let new_line = format!("{key} = {value}");

    let mut output: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut source_line_idx: Option<usize> = None;
    let mut in_source = false;

    for line in content.lines() {
        if let Some(header) = table_header(line) {
            in_source = is_source_header(header);
            if in_source && source_line_idx.is_none() {
                source_line_idx = Some(output.len());
            }
        }

        if in_source && is_active_key(line, key) {
            output.push(new_line.clone());
            replaced = true;
        } else {
            output.push(line.to_string());
        }
    }

    if !replaced {
        if let Some(i) = source_line_idx {
            output.insert(i + 1, new_line);
        } else {
            // No [source] section in the file; append one followed by the key.
            output.push("[source]".to_string());
            output.push(new_line);
        }
    }

    let mut result = output.join("\n");
    result.push('\n');
    result
}

/// A TOML basic string holding `s`, always on one line: `\` and `"` are
/// escaped, and so is every control character, which TOML refuses raw (a raw
/// line break would also split the line this module edits by line).
fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // plugin_name — INIT-11
    // -------------------------------------------------------------------------

    #[test]
    fn plugin_name_namespace_flag_wins() {
        assert_eq!(
            plugin_name("dirname", Some("prefix"), Some("ns-flag")),
            "ns-flag"
        );
    }

    #[test]
    fn plugin_name_mindfile_prefix_without_flag() {
        assert_eq!(plugin_name("dirname", Some("mypkg"), None), "mypkg");
    }

    #[test]
    fn plugin_name_falls_back_to_dirname() {
        assert_eq!(plugin_name("myrepo", None, None), "myrepo");
    }

    #[test]
    fn plugin_name_namespace_flag_overrides_prefix() {
        assert_eq!(
            plugin_name("dir", Some("pkg"), Some("override")),
            "override"
        );
    }

    // -------------------------------------------------------------------------
    // render_marketplace_json — INIT-10
    // -------------------------------------------------------------------------

    #[test]
    fn render_marketplace_json_no_skills_key() {
        let out = render_marketplace_json("myplugin", "A plugin", None);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["name"], "myplugin");
        let plugin = &v["plugins"][0];
        assert_eq!(plugin["name"], "myplugin");
        assert_eq!(plugin["source"], ".");
        assert_eq!(plugin["description"], "A plugin");
        assert!(
            plugin.get("skills").is_none(),
            "skills key must be absent when None"
        );
    }

    #[test]
    fn render_marketplace_json_with_skills() {
        let skills = vec!["review".to_string(), "build".to_string()];
        let out = render_marketplace_json("pkg", "desc", Some(&skills));
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let s = v["plugins"][0]["skills"].as_array().expect("skills array");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "review");
        assert_eq!(s[1], "build");
    }

    #[test]
    fn render_marketplace_json_with_empty_skills() {
        let skills: Vec<String> = vec![];
        let out = render_marketplace_json("p", "d", Some(&skills));
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let s = v["plugins"][0]["skills"]
            .as_array()
            .expect("skills key present even if empty");
        assert!(s.is_empty());
    }

    #[test]
    fn render_marketplace_json_name_in_both_top_and_entry() {
        let out = render_marketplace_json("foo", "bar", None);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["name"], "foo", "top-level name");
        assert_eq!(v["plugins"][0]["name"], "foo", "entry name");
    }

    #[test]
    fn render_marketplace_json_ends_with_newline() {
        let out = render_marketplace_json("n", "d", None);
        assert!(out.ends_with('\n'), "output must end with newline");
    }

    // -------------------------------------------------------------------------
    // patch_source_meta — INIT-12
    // -------------------------------------------------------------------------

    #[test]
    fn patch_source_meta_inserts_flat_skills_into_scaffold() {
        // No existing file: starts from scaffold. flat-skills is absent in the
        // scaffold, so it is inserted right after [source].
        let out = patch_source_meta(None, true, None);
        assert!(
            out.contains("flat-skills = true"),
            "must insert flat-skills: {out}"
        );
        // The rest of the scaffold content is preserved.
        assert!(out.contains("[source]"), "must keep [source] header: {out}");
        assert!(
            out.contains("description = \"\""),
            "must keep description line: {out}"
        );
    }

    #[test]
    fn patch_source_meta_inserts_namespace_into_scaffold() {
        // spec: DSC-82 / INIT-11 -- patch_source_meta must write `namespace =` as
        // the active key (`prefix` is the deprecated read alias).
        let out = patch_source_meta(None, false, Some("mypkg"));
        assert!(
            out.contains("namespace = \"mypkg\""),
            "must insert namespace key: {out}"
        );
        // Must not emit `prefix =` as an active line (only `namespace =` is written).
        let active_prefix = out
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && l.trim_start().starts_with("prefix ="));
        assert!(
            !active_prefix,
            "active key must be `namespace`, not `prefix`: {out}"
        );
    }

    #[test]
    fn patch_source_meta_replaces_existing_flat_skills_false() {
        let existing = "[source]\nflat-skills = false\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), true, None);
        assert!(
            out.contains("flat-skills = true"),
            "must replace false with true: {out}"
        );
        assert!(
            !out.contains("flat-skills = false"),
            "old value must be gone: {out}"
        );
    }

    #[test]
    fn patch_source_meta_replaces_existing_prefix_key() {
        // spec: DSC-82 -- when the existing mind.toml uses the deprecated
        // `prefix = "old"`, patch_source_meta must normalize it to
        // `namespace = "new"` (single key, no duplication).
        let existing = "[source]\nprefix = \"old\"\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), false, Some("new"));
        assert!(
            out.contains("namespace = \"new\""),
            "must write namespace = new: {out}"
        );
        // The old prefix key must be gone (replaced, not left alongside).
        assert!(
            !out.contains("prefix = \"old\""),
            "old prefix = old must be gone: {out}"
        );
        // No stray active `prefix =` line.
        let active_prefix = out
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && l.trim_start().starts_with("prefix ="));
        assert!(!active_prefix, "no active prefix key must remain: {out}");
    }

    #[test]
    fn patch_source_meta_inserts_after_source_header() {
        let existing = "[source]\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), true, None);
        // flat-skills must appear before description (inserted right after [source]).
        let flat_pos = out.find("flat-skills").unwrap();
        let desc_pos = out.find("description").unwrap();
        assert!(
            flat_pos < desc_pos,
            "flat-skills must appear before description: {out}"
        );
    }

    #[test]
    fn patch_source_meta_comment_lines_not_treated_as_active() {
        // A commented prefix line is not treated as an active key; the new
        // namespace is inserted as an active line (INIT-11 write key is
        // `namespace`).
        let existing = "[source]\n# prefix = \"old\"\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), false, Some("real"));
        assert!(
            out.contains("namespace = \"real\""),
            "must insert active namespace key: {out}"
        );
        assert!(
            out.contains("# prefix = \"old\""),
            "must keep comment line: {out}"
        );
    }

    #[test]
    fn patch_source_meta_both_flags() {
        // Both flags: flat-skills and namespace (INIT-11/INIT-12 write behavior).
        let out = patch_source_meta(None, true, Some("mypkg"));
        assert!(
            out.contains("flat-skills = true"),
            "must have flat-skills: {out}"
        );
        assert!(
            out.contains("namespace = \"mypkg\""),
            "must emit namespace key: {out}"
        );
    }

    #[test]
    fn patch_source_meta_no_flags_returns_scaffold() {
        // Neither flag set: content is unchanged (just the scaffold).
        let out = patch_source_meta(None, false, None);
        assert_eq!(out, scaffold(), "no-op must return the scaffold unchanged");
    }

    #[test]
    fn scaffold_declares_the_running_release_as_min_mind_version() {
        // spec: INIT-13
        let out = scaffold();
        let line = format!(
            "min-mind-version = \"{}\"",
            crate::mindfile::running_release_version()
        );
        assert!(out.contains(&line), "{out}");
        assert!(!out.contains("{version}"), "{out}");
        // The scaffold must load: the value passes the DSC-40 validator even
        // on a `-dev` build.
        let dir = std::env::temp_dir().join(format!("mind-scaffold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), &out).unwrap();
        let loaded = crate::mindfile::MindToml::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let loaded = loaded.expect("scaffold parses").expect("present");
        assert!(crate::mindfile::unmet_min_version(Some(&loaded)).is_none());
    }

    #[test]
    fn running_release_version_drops_the_prerelease_suffix() {
        // spec: INIT-13
        let v = crate::mindfile::running_release_version();
        assert!(crate::mindfile::is_plausible_version(v), "{v}");
        assert!(env!("CARGO_PKG_VERSION").starts_with(v));
    }

    #[test]
    fn patch_source_meta_existing_content_no_flags() {
        let existing = "[source]\ndescription = \"existing\"\n";
        let out = patch_source_meta(Some(existing), false, None);
        assert_eq!(
            out, existing,
            "no-op must return existing content unchanged"
        );
    }

    #[test]
    fn toml_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_string("foo"), "\"foo\"");
        assert_eq!(toml_string("foo\"bar"), "\"foo\\\"bar\"");
        assert_eq!(toml_string("foo\\bar"), "\"foo\\\\bar\"");
    }

    // -------------------------------------------------------------------------
    // set_key — M7: missing [source] section
    // -------------------------------------------------------------------------

    /// spec: M7 — when the file has no [source] header, set_key must append
    /// one before inserting the key so the value lands in the right TOML scope.
    #[test]
    fn set_key_creates_source_section_when_absent() {
        // A mind.toml that only has [[items]] and no [source] block.
        let content = "[[items]]\nkind = \"skill\"\nname = \"foo\"\n";
        let out = set_key(content, "prefix", "\"mypkg\"");

        // The [source] header must now be present.
        assert!(
            out.contains("[source]"),
            "must create [source] section: {out}"
        );
        // The key must be present.
        assert!(
            out.contains("prefix = \"mypkg\""),
            "must insert key under [source]: {out}"
        );
        // [source] must precede the key.
        let source_pos = out.find("[source]").unwrap();
        let key_pos = out.find("prefix = \"mypkg\"").unwrap();
        assert!(source_pos < key_pos, "[source] must precede the key: {out}");
        // Original [[items]] content must be preserved.
        assert!(
            out.contains("[[items]]"),
            "must keep [[items]] section: {out}"
        );
        // Output must end with a newline.
        assert!(out.ends_with('\n'), "must end with newline: {out:?}");
    }

    /// spec: M7 regression — when [source] already exists, set_key inserts the
    /// key right after the header (existing happy path must not regress).
    #[test]
    fn set_key_inserts_after_existing_source_section() {
        let content = "[source]\ndescription = \"\"\n";
        let out = set_key(content, "prefix", "\"mypkg\"");

        assert!(out.contains("prefix = \"mypkg\""), "must insert key: {out}");
        // The key must appear after [source].
        let source_pos = out.find("[source]").unwrap();
        let key_pos = out.find("prefix = \"mypkg\"").unwrap();
        assert!(
            source_pos < key_pos,
            "key must appear after [source]: {out}"
        );
        // description line must still be present.
        assert!(
            out.contains("description = \"\""),
            "must preserve existing lines: {out}"
        );
    }

    #[test]
    fn patch_source_meta_touches_only_the_source_table() {
        // spec: INIT-12 -- a `[[discover.sources]]` entry's own `namespace`,
        // `flat-skills` and `prefix`-like keys are not the `[source]` keys.
        let content = "[source]\ndescription = \"d\"\nprefix = \"old\"\n\n\
                       [[discover.sources]]\nsource = \"a/b\"\nnamespace = \"keep\"\n\
                       flat-skills = false\n";
        let out = patch_source_meta(Some(content), true, Some("new"));
        let doc: toml::Value = toml::from_str(&out).expect(&out);
        assert_eq!(doc["source"]["namespace"].as_str(), Some("new"), "{out}");
        assert_eq!(doc["source"]["flat-skills"].as_bool(), Some(true), "{out}");
        assert!(doc["source"].get("prefix").is_none(), "{out}");
        let entry = &doc["discover"]["sources"][0];
        assert_eq!(entry["namespace"].as_str(), Some("keep"), "{out}");
        assert_eq!(entry["flat-skills"].as_bool(), Some(false), "{out}");
    }

    #[test]
    fn patch_source_meta_finds_a_source_header_with_a_comment() {
        // spec: INIT-12 -- `[source]  # note` is the `[source]` header; no
        // second table is appended.
        let content = "[source]  # metadata\ndescription = \"d\"\n";
        let out = patch_source_meta(Some(content), false, Some("ns"));
        let doc: toml::Value = toml::from_str(&out).expect(&out);
        assert_eq!(doc["source"]["namespace"].as_str(), Some("ns"), "{out}");
        assert_eq!(doc["source"]["description"].as_str(), Some("d"), "{out}");
    }

    // ---- properties ---------------------------------------------------------

    use crate::test_gen::{Gen, cases};

    /// A one-line TOML string value holding arbitrary text.
    fn gen_toml_string(g: &mut Gen) -> String {
        let s = g.any_string(0, 10).replace(['\n', '\r'], "");
        toml::Value::String(s).to_string()
    }

    /// A `[source]` header in one of the spellings TOML allows.
    fn gen_source_header(g: &mut Gen) -> String {
        g.pick(&[
            "[source]",
            "[source]  # metadata",
            "[ source ]",
            "[\"source\"]",
            "  [source]",
            "[source]#x",
        ])
        .to_string()
    }

    /// The lines of a `[source]` table: any of its keys, active or commented.
    fn gen_source_table(g: &mut Gen) -> Vec<String> {
        let mut lines = vec![gen_source_header(g)];
        for key in ["description", "prefix", "namespace", "flat-skills", "lobe"] {
            match g.below(3) {
                0 => {}
                1 => lines.push(format!("# {key} = \"commented\"")),
                _ if key == "flat-skills" => {
                    lines.push(format!("{key} = {}", g.pick(&["true", "false"])))
                }
                _ => lines.push(format!(
                    "{key}{}= {}",
                    g.pick(&[" ", "  ", ""]),
                    gen_toml_string(g)
                )),
            }
        }
        lines
    }

    /// Tables other than `[source]`, several of which spell `[source]`'s keys.
    /// A plain table (`[discover]`, `[profiles]`) appears at most once; `used`
    /// tracks them.
    fn gen_other_table(g: &mut Gen, used: &mut Vec<usize>) -> Vec<String> {
        let mut pick = g.below(4);
        if pick >= 2 {
            if used.contains(&pick) {
                pick = g.below(2);
            } else {
                used.push(pick);
            }
        }
        match pick {
            0 => vec![
                "[[discover.sources]]".into(),
                format!("source = {}", gen_toml_string(g)),
                format!("namespace = {}", gen_toml_string(g)),
                format!("prefix = {}", gen_toml_string(g)),
                "flat-skills = false".into(),
            ],
            1 => vec![
                "[[items]]".into(),
                "kind = \"skill\"".into(),
                format!("name = {}", gen_toml_string(g)),
            ],
            2 => vec![
                "[discover]  # globs".into(),
                format!("skills = [{}]", gen_toml_string(g)),
            ],
            _ => vec![
                "[profiles]".into(),
                format!("core = [{}]", gen_toml_string(g)),
            ],
        }
    }

    /// A `mind.toml`: a comment preamble, at most one `[source]` table, and
    /// any number of other tables, in any order.
    fn gen_mind_toml(g: &mut Gen) -> String {
        let mut lines: Vec<String> = Vec::new();
        if g.one_in(2) {
            lines.push("# a mind.toml".into());
        }
        let n = g.below(4);
        let source_at = (!g.one_in(4)).then(|| g.below(n + 1));
        let mut used = Vec::new();
        for i in 0..=n {
            if source_at == Some(i) {
                lines.extend(gen_source_table(g));
            }
            if i < n {
                lines.extend(gen_other_table(g, &mut used));
            }
            if g.one_in(2) {
                lines.push(String::new());
            }
        }
        let mut text = lines.join("\n");
        if !g.one_in(4) {
            text.push('\n');
        }
        text
    }

    // spec: INIT-12 DSC-82
    #[test]
    fn patch_source_meta_sets_only_the_source_keys() {
        cases(0x5caf_0001, 5_000, |g| {
            let input = gen_mind_toml(g);
            let before: toml::Table =
                toml::from_str(&input).unwrap_or_else(|e| panic!("{input}\n{e}"));
            let flat = g.one_in(2);
            let ns = g.one_in(4).then(|| g.any_string(0, 8));
            let out = patch_source_meta(Some(&input), flat, ns.as_deref());
            let after: toml::Table = toml::from_str(&out)
                .unwrap_or_else(|e| panic!("patched output must parse:\n{input}\n---\n{out}\n{e}"));

            let mut want = before.clone();
            let source = want
                .entry("source")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .unwrap();
            if flat {
                source.insert("flat-skills".into(), toml::Value::Boolean(true));
            }
            if let Some(ns) = &ns {
                source.insert("namespace".into(), toml::Value::String(ns.clone()));
                source.remove("prefix");
            }
            if !flat && ns.is_none() {
                want = before;
            }
            assert_eq!(after, want, "\n{input}\n---\n{out}");
            for line in input.lines().filter(|l| l.trim_start().starts_with('#')) {
                assert!(
                    out.lines().any(|o| o == line),
                    "comment {line:?} kept:\n{out}"
                );
            }
        });
    }
}
