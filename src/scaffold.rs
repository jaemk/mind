//! Pure helpers for `init-source` scaffolding (INIT-10, INIT-11, INIT-12).
//!
//! Three public functions cover the three new behaviors; no filesystem I/O here.

// The same scaffold text written by INIT-3; kept here so `patch_source_meta`
// can start from it when no mind.toml exists yet.
pub(crate) const SCAFFOLD: &str = concat!(
    "[source]\n",
    "description = \"\"   # what this source offers\n",
    "# prefix = \"prefix\"   # namespace items as prefix:<name>\n",
    "\n",
    "# Declare hooks that run when a consumer melds or unmelds this source.\n",
    "# Remove the leading `# ` to enable a hook.\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make install\"         # shell command to run\n",
    "# name = \"Build\"               # optional label shown in the prompt\n",
    "# event = \"install\"             # \"install\" (default) or \"uninstall\"\n",
    "# optional = false              # false = required (default). optional only lets the\n",
    "#                               # user decline running it; a failure always aborts.\n",
    "#\n",
    "# [[hooks]]\n",
    "# run = \"make clean\"            # cleanup hook run at unmeld time\n",
    "# name = \"Cleanup\"\n",
    "# event = \"uninstall\"\n",
    "# optional = true               # the user may decline this step (its failure still aborts)\n",
);

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
    let base = existing_content.unwrap_or(SCAFFOLD);
    let mut result = base.to_string();

    if flat_skills {
        result = set_key(&result, "flat-skills", "true");
    }
    if let Some(ns) = namespace {
        let value = toml_string(ns);
        result = set_key(&result, "prefix", &value);
    }

    result
}

/// Replace the active `<key> = ...` line in `content`, or insert it after
/// the `[source]` header. Non-active (commented) lines are left untouched.
fn set_key(content: &str, key: &str, value: &str) -> String {
    let new_line = format!("{key} = {value}");
    let key_eq = format!("{key} =");
    let key_eq_nospace = format!("{key}=");

    let mut output: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut source_line_idx: Option<usize> = None;

    for line in content.lines() {
        if line.trim() == "[source]" && source_line_idx.is_none() {
            source_line_idx = Some(output.len());
        }
        let trimmed = line.trim_start();
        let is_active_key = !trimmed.starts_with('#')
            && (trimmed.starts_with(&key_eq) || trimmed.starts_with(&key_eq_nospace));

        if is_active_key {
            output.push(new_line.clone());
            replaced = true;
        } else {
            output.push(line.to_string());
        }
    }

    if !replaced {
        let insert_at = source_line_idx.map(|i| i + 1).unwrap_or(0);
        output.insert(insert_at, new_line);
    }

    let mut result = output.join("\n");
    result.push('\n');
    result
}

/// Produce a TOML double-quoted string value with basic escaping.
fn toml_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
    fn patch_source_meta_inserts_prefix_into_scaffold() {
        let out = patch_source_meta(None, false, Some("mypkg"));
        assert!(
            out.contains("prefix = \"mypkg\""),
            "must insert prefix: {out}"
        );
        // The commented-out generic prefix line is preserved (it is a comment,
        // not the active key).
        assert!(
            out.contains("# prefix = \"prefix\""),
            "must keep commented prefix: {out}"
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
    fn patch_source_meta_replaces_existing_prefix() {
        let existing = "[source]\nprefix = \"old\"\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), false, Some("new"));
        assert!(
            out.contains("prefix = \"new\""),
            "must replace old prefix: {out}"
        );
        assert!(
            !out.contains("prefix = \"old\""),
            "old prefix must be gone: {out}"
        );
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
        // prefix is inserted as an active line.
        let existing = "[source]\n# prefix = \"old\"\ndescription = \"\"\n";
        let out = patch_source_meta(Some(existing), false, Some("real"));
        assert!(
            out.contains("prefix = \"real\""),
            "must insert active prefix: {out}"
        );
        assert!(
            out.contains("# prefix = \"old\""),
            "must keep comment line: {out}"
        );
    }

    #[test]
    fn patch_source_meta_both_flags() {
        let out = patch_source_meta(None, true, Some("mypkg"));
        assert!(
            out.contains("flat-skills = true"),
            "must have flat-skills: {out}"
        );
        assert!(
            out.contains("prefix = \"mypkg\""),
            "must have prefix: {out}"
        );
    }

    #[test]
    fn patch_source_meta_no_flags_returns_scaffold() {
        // Neither flag set: content is unchanged (just the scaffold).
        let out = patch_source_meta(None, false, None);
        assert_eq!(out, SCAFFOLD, "no-op must return the scaffold unchanged");
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
}
