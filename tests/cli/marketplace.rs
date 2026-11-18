//! Claude plugin and marketplace sources (spec/marketplace.md).

use super::*;

#[test]
fn marketplace_plugin_meld_discovers_skill_and_agent() {
    // spec: MKT-1, MKT-3
    // A single-plugin source (.claude-plugin/plugin.json) feeds the normal
    // catalog -> store -> symlink pipeline.  Only skill and agent kinds are
    // produced; no rule or tool items come from a plugin.
    //
    // Note: agents appear in probe with the plugin-name prefix in their key
    // (agent:acme-tools:helper), matching the existing NS-40 behavior where
    // probe uses the effective name but the installed LINK is the bare
    // frontmatter name (agents/helper.md).
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:acme-tools:greet"),
        "plugin skill must appear in probe: {}",
        probe.stdout
    );
    // Agent probe key uses the effective (prefixed) name per NS-40/MKT-5.
    assert!(
        probe.stdout.contains("agent:acme-tools:helper"),
        "plugin agent must appear in probe: {}",
        probe.stdout
    );
    // spec: MKT-18 -- the plugin's commands/ maps to the command kind, under
    // the plugin-name prefix like its other items (MKT-5).
    assert!(
        probe.stdout.contains("command:acme-tools:hello"),
        "plugin command must appear in probe: {}",
        probe.stdout
    );
    // MKT-3: no rule or tool items
    assert!(
        !probe.stdout.contains("rule:"),
        "no rule items from a plugin: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("tool:"),
        "no tool items from a plugin: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_learn_installs_and_links() {
    // spec: MKT-1
    // `learn` installs a plugin skill through the normal store+symlink pipeline.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);

    // The skill directory is symlinked into the lobe under its effective name.
    let link = sb.claude_home.join("skills/acme-tools:greet");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "symlink must exist at claude_home/skills/acme-tools:greet"
    );
}

#[test]
fn marketplace_plugin_skipped_components_note() {
    // spec: MKT-4 MKT-18
    // An unsupported component kind (hooks/) is not installed; meld prints a
    // count note so the user is not misled into thinking the plugin is fully
    // represented. The fixture's commands/ is NOT among them: it installs.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("not installed (no mind equivalent)"),
        "meld must print the skipped-components note: {combined}"
    );
    assert!(
        combined.contains("hook"),
        "skipped-components note must name the unsupported kind: {combined}"
    );
    let note = combined
        .lines()
        .find(|l| l.contains("not installed (no mind equivalent)"))
        .expect("the note line");
    assert!(
        !note.contains("command"),
        "a plugin's commands are installed, so the skipped note must not name \
         them: {note}"
    );
    // spec: WF-40 -- and the same for its workflows.
    assert!(
        !note.contains("workflow"),
        "a plugin's workflows are installed, so the skipped note must not name \
         them: {note}"
    );
}

#[test]
fn marketplace_plugin_command_installs_and_links() {
    // spec: MKT-18 CMD-5
    // A plugin's command rides the normal store+symlink pipeline, linking under
    // the plugin-name prefix at commands/<prefix>:<name>.md.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&["learn", "command:acme-tools:hello"]);
    assert!(r.success, "learn command failed: {} {}", r.stdout, r.stderr);

    let link = sb.claude_home.join("commands/acme-tools:hello.md");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "the plugin command must be linked at commands/acme-tools:hello.md"
    );
    assert!(
        std::fs::read_to_string(&link)
            .expect("read the linked command")
            .contains("Greet the current project"),
        "the linked file must be the plugin's command"
    );
}

#[test]
fn marketplace_plugin_name_is_default_prefix_for_skills() {
    // spec: MKT-5
    // The plugin.json `name` is the default namespace prefix for skills.
    // For agents, NS-40 specifies the lobe LINK is always the bare frontmatter
    // `name` (not `plugin:agent`), even though probe still shows the effective
    // (prefixed) key.  Verify both: skill prefix in probe, bare link on disk.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let probe = sb.mind(&["probe"]);
    // Skill carries the plugin name as prefix.
    assert!(
        probe.stdout.contains("skill:acme-tools:greet"),
        "skill must be prefixed with the plugin name: {}",
        probe.stdout
    );

    // Install the agent and verify its lobe link is the bare frontmatter name.
    // The probe key is `agent:acme-tools:helper` (effective name), so we learn
    // by that key.
    let r = sb.mind(&["learn", "acme-tools:helper"]);
    assert!(r.success, "learn agent failed: {} {}", r.stdout, r.stderr);
    // NS-40/MKT-5: the lobe link is under the bare harness name, not the prefix.
    assert!(
        sb.claude_home.join("agents/helper.md").exists(),
        "agent link must be at agents/helper.md (bare harness name, not prefixed)"
    );
    assert!(
        !sb.claude_home.join("agents/acme-tools:helper.md").exists(),
        "no prefixed agent link must exist"
    );
}

#[test]
fn marketplace_plugin_namespace_override_sets_prefix() {
    // spec: MKT-5
    // `meld --namespace z` overrides the plugin-name prefix; skills install as
    // z:<bare>.  The agent LINK is still bare (NS-40: lobe link ignores the
    // prefix); its probe key reflects the override (agent:z:helper).
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "z", "--link-only"])
            .success
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:z:greet"),
        "consumer --namespace must override the plugin-name prefix: {}",
        probe.stdout
    );

    // Install the agent; its lobe link must remain bare regardless of the
    // consumer namespace.
    let r = sb.mind(&["learn", "z:helper"]);
    assert!(r.success, "learn agent failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("agents/helper.md").exists(),
        "agent link must be bare (agents/helper.md) even with a consumer namespace"
    );
    assert!(
        !sb.claude_home.join("agents/z:helper.md").exists(),
        "no prefixed agent link must exist"
    );
}

#[test]
fn marketplace_plugin_namespace_empty_clears_prefix() {
    // spec: MKT-5
    // `meld --namespace ''` (empty) removes the plugin-name prefix.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "", "--link-only"])
            .success
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:greet"),
        "empty --namespace must clear the plugin-name prefix (skill is bare): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:acme-tools:greet"),
        "plugin-name prefix must not appear after --namespace '': {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_description_and_version_recorded() {
    // spec: MKT-6
    // The plugin.json `description` is recorded on the source and appears in
    // `recall --sources`.  The `version` is stored and visible in JSON output.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // Description surfaces in the plain-text recall --sources output.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("Acme developer tools plugin"),
        "plugin description must appear in recall --sources: {}",
        sources.stdout
    );

    // plugin_version is recorded; confirm via --json output.
    let jsrc = sb.mind(&["recall", "--sources", "--json"]);
    assert!(jsrc.success, "{}", jsrc.stderr);
    assert!(
        jsrc.stdout.contains("1.0.0"),
        "plugin_version must be present in JSON output: {}",
        jsrc.stdout
    );
}

#[test]
fn marketplace_plugin_description_strips_bidi_directional_and_zero_width() {
    // spec: MKT-9, DSC-69 -- a plugin.json `description` is untrusted,
    // attacker-controlled content. It used to reach display through a private
    // copy of `strip_ansi` in commands/ that only blocked the bidi-override
    // range and line/paragraph separators; the shared, hardened
    // `crate::sanitize::strip_ansi` (CLI-224) additionally blocks the
    // directional marks U+200E/U+200F/U+061C and the zero-width characters
    // U+200B/U+2060/U+FEFF. Now that commands/ routes through the one
    // shared implementation, all of those must be stripped here too, in both
    // plain-text and `--json` output.
    let sb = Sandbox::bare("quote-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        "{\"name\":\"quoteplugin\",\"version\":\"1.0\",\
         \"description\":\"Evil \u{202E}RTL\u{200E} zerowidth\u{200B} end\"}",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("Evil RTL zerowidth end"),
        "the sanitized description text must survive: {}",
        sources.stdout
    );
    assert!(
        !sources.stdout.contains('\u{202E}')
            && !sources.stdout.contains('\u{200E}')
            && !sources.stdout.contains('\u{200B}'),
        "the bidi override, directional mark, and zero-width character must \
         all be stripped from plain-text output: {:?}",
        sources.stdout
    );

    let jsrc = sb.mind(&["recall", "--sources", "--json"]);
    assert!(jsrc.success, "{}", jsrc.stderr);
    assert!(
        jsrc.stdout.contains("Evil RTL zerowidth end"),
        "the sanitized description text must survive in --json: {}",
        jsrc.stdout
    );
    assert!(
        !jsrc.stdout.contains('\u{202E}')
            && !jsrc.stdout.contains('\u{200E}')
            && !jsrc.stdout.contains('\u{200B}'),
        "the bidi override, directional mark, and zero-width character must \
         all be stripped from --json output: {:?}",
        jsrc.stdout
    );
}

#[test]
fn marketplace_catalog_melds_in_repo_plugins() {
    // spec: MKT-7 MKT-14
    // A .claude-plugin/marketplace.json catalog scans each listed in-repo
    // plugin as catalog items of the parent source (MKT-14); both plugins'
    // items appear in probe. Installing an item works through the normal
    // `learn` path.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The parent source is registered. Under MKT-14 in-repo plugins are
    // items of the parent source, not separate sub-sources.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("marketplace-catalog"),
        "catalog source must appear: {}",
        sources.stdout
    );

    // Both plugins' items are available for install via probe.
    // Beta's agent probe key is `agent:beta:two` (effective/prefixed name per
    // NS-40), but its lobe LINK lands at agents/two.md (bare harness name).
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha:one"),
        "alpha's skill must appear in probe: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta:two"),
        "beta's agent must appear in probe (prefixed effective name): {}",
        probe.stdout
    );
    // spec: MKT-18 -- an in-repo entry's commands/ is always scanned, like its
    // agents, and namespaced by the entry name (MKT-8).
    assert!(
        probe.stdout.contains("command:beta:ship"),
        "beta's command must appear in probe: {}",
        probe.stdout
    );
    // spec: WF-40 -- and so is its workflows/, on this path as well as the
    // directly-melded plugin one.
    assert!(
        probe.stdout.contains("workflow:beta:ship-it"),
        "beta's workflow must appear in probe: {}",
        probe.stdout
    );

    // Items from sub-sources are installable through the normal `learn` path.
    let r = sb.mind(&["learn", "alpha:one"]);
    assert!(
        r.success,
        "learn alpha:one failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/alpha:one").exists(),
        "skill alpha:one must be symlinked into the lobe"
    );

    // Beta's agent links under the bare frontmatter name (NS-40/MKT-5).
    let r = sb.mind(&["learn", "beta:two"]);
    assert!(
        r.success,
        "learn beta:two failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("agents/two.md").exists(),
        "beta's agent link must be at agents/two.md (bare harness name)"
    );

    // spec: MKT-18 -- and its command installs like any other item.
    let r = sb.mind(&["learn", "command:beta:ship"]);
    assert!(
        r.success,
        "learn beta's command failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("commands/beta:ship.md").exists(),
        "beta's command link must be at commands/beta:ship.md"
    );

    // spec: WF-40 WF-42 -- the workflow installs the same way, and its
    // `{{ns:}}` name expands to the entry-namespaced spelling.
    let r = sb.mind(&["learn", "workflow:beta:ship-it"]);
    assert!(
        r.success,
        "learn beta's workflow failed: {} {}",
        r.stdout, r.stderr
    );
    let link = sb.claude_home.join("workflows/beta:ship-it.js");
    assert!(
        link.exists(),
        "beta's workflow link must be at workflows/beta:ship-it.js"
    );
    let body = std::fs::read_to_string(&link).expect("read the linked workflow");
    assert!(
        body.contains("name: 'beta:ship-it'"),
        "the token must expand to the entry-prefixed name: {body}"
    );
}

#[test]
fn marketplace_catalog_probe_hint_fires() {
    // spec: MKT-7
    // After melding a marketplace catalog, `maybe_probe_hint` prints the
    // curated-source hint (DSC-56) so the user knows to browse with `probe`.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind probe"),
        "meld of a marketplace catalog must print the probe hint: {combined}"
    );
}

#[test]
fn marketplace_catalog_external_plugin_registers() {
    // spec: MKT-7
    // A marketplace entry pointing at an external git source (local path via
    // file:// URL in the test) melds it as a nested sub-source tracking its
    // own commit, mirroring the [discover].sources external-source behavior.
    let extplugin = Sandbox::bare("extplugin");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1","description":"External test plugin"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );

    // A catalog that references the external plugin via file:// URL
    // (file:// is detected as External by is_external_string -> contains "://").
    let catalog = Sandbox::bare("ext-catalog");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"ext-market","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );

    let cat_spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &cat_spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The external plugin is registered as a sub-source.
    let sources = catalog.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("extplugin"),
        "external plugin sub-source must be registered: {}",
        sources.stdout
    );

    // Its skill is discoverable.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:ext:extskill"),
        "external plugin's skill must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_yes_auto_installs_in_repo_plugins() {
    // spec: MKT-7
    // MKT-7: a marketplace catalog's in-repo plugins are offered for install on
    // meld like the catalog's own items (CLI-23). With `--yes` (non-TTY), they
    // install automatically -- no explicit `learn` needed.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);

    // Both in-repo plugins' items were installed automatically.
    assert!(
        sb.claude_home.join("skills/alpha:one").exists(),
        "alpha's skill must be auto-installed under --yes: lobe = {}",
        sb.claude_home.display()
    );
    assert!(
        sb.claude_home.join("agents/two.md").exists(),
        "beta's agent must be auto-installed under --yes (bare harness name)"
    );
}

#[test]
fn marketplace_external_plugin_installs_only_under_recursive() {
    // spec: MKT-7
    // DSC-54/DSC-55 via MKT-7: an EXTERNAL marketplace plugin is register-only on
    // a plain `--yes` meld (left available), and installs only under
    // `--recursive`, mirroring a `[discover].sources` external nested source.
    let extplugin = Sandbox::bare("extplugin2");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1","description":"External test plugin"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );
    let catalog = Sandbox::bare("ext-catalog2");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"ext-market","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );
    let cat_spec = catalog.source_spec();

    // Plain --yes meld: the external plugin registers but is NOT installed.
    let r = catalog.mind(&["meld", &cat_spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    assert!(
        !catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin must be register-only without --recursive"
    );

    // Re-meld with --recursive --yes: now the external plugin's item installs.
    let r = catalog.mind(&["meld", &cat_spec, "--recursive", "--yes"]);
    assert!(
        r.success,
        "remeld --recursive --yes failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin's skill must install under --recursive"
    );
}

#[test]
fn marketplace_entry_name_wins_over_plugin_json_name() {
    // spec: MKT-8
    // The marketplace entry `name` is used as the alias (namespace prefix) for
    // that plugin's items, overriding whatever name the in-repo plugin.json
    // declares.  The entry name is authoritative.
    let catalog = Sandbox::bare("mkt8-catalog");
    // Build an in-repo plugin whose plugin.json says "original-name".
    catalog.write_and_commit(
        "plugins/myplugin/.claude-plugin/plugin.json",
        r#"{"name":"original-name","version":"0.1"}"#,
    );
    catalog.write_and_commit(
        "plugins/myplugin/skills/theskill/SKILL.md",
        "---\nname: theskill\ndescription: A skill\n---\n# theskill\n",
    );
    // The catalog entry uses "override-name" as the name (MKT-8: entry wins).
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"test","plugins":[{"name":"override-name","source":"./plugins/myplugin"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // The skill must appear under "override-name", not "original-name".
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:override-name:theskill"),
        "entry name must override plugin.json name as the prefix: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:original-name:theskill"),
        "plugin.json name must not appear as prefix when entry overrides it: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_unsafe_in_repo_path_fails_meld() {
    // spec: MKT-9
    // An in-repo source path of "../escape" contains ".." and must be rejected
    // at meld time before any filesystem traversal happens.
    let catalog = Sandbox::bare("unsafe-mkt");
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"bad","plugins":[{"name":"evil","source":"../escape"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(!r.success, "unsafe path must cause meld to fail");
    assert!(
        r.stderr.contains("unsafe") || r.stderr.contains("..") || r.stderr.contains("escape"),
        "error must describe the path safety violation: {}",
        r.stderr
    );
}

#[test]
fn marketplace_malformed_plugin_json_fails_meld() {
    // spec: MKT-9
    // A .claude-plugin/plugin.json that is not valid JSON must fail the meld
    // with a clear error rather than silently producing zero items.
    let sb = Sandbox::bare("bad-plugin-json");
    sb.write_and_commit(".claude-plugin/plugin.json", "{not valid json at all");

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success, "malformed plugin.json must cause meld to fail");
    assert!(
        r.stderr.contains("plugin.json") || r.stderr.contains("invalid"),
        "error must indicate the invalid plugin.json: {}",
        r.stderr
    );
}

#[test]
fn marketplace_malformed_marketplace_json_fails_meld() {
    // spec: MKT-9
    // A .claude-plugin/marketplace.json that is not valid JSON must fail meld.
    let sb = Sandbox::bare("bad-market-json");
    sb.write_and_commit(".claude-plugin/marketplace.json", "{bad json");

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "malformed marketplace.json must cause meld to fail"
    );
    assert!(
        r.stderr.contains("marketplace.json") || r.stderr.contains("invalid"),
        "error must indicate the invalid marketplace.json: {}",
        r.stderr
    );
}

#[test]
fn marketplace_ansi_escape_stripped_in_recall_sources() {
    // spec: MKT-9
    // Descriptions taken from a plugin.json have ANSI escape sequences stripped
    // before display, so a malicious or miscoded description cannot corrupt the
    // terminal via `recall --sources`.
    let sb = Sandbox::bare("ansi-plugin");
    // ESC [ 3 1 m = red-color CSI sequence; should be stripped.
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        "{\"name\":\"ansi-p\",\"description\":\"\\u001b[31mred text\\u001b[0m safe\"}",
    );

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    // The ESC byte (0x1b) must not appear in the output.
    assert!(
        !sources.stdout.contains('\x1b'),
        "ANSI escape bytes must be stripped from recall --sources output: {:?}",
        sources.stdout
    );
    // The visible text portion must still appear (the safe suffix after stripping).
    assert!(
        sources.stdout.contains("safe") || sources.stdout.contains("red text"),
        "stripped text must still be visible: {}",
        sources.stdout
    );
}

#[test]
fn marketplace_plugin_origin_label_in_recall_sources() {
    // spec: MKT-10
    // A source whose items came from a .claude-plugin/plugin.json is labelled
    // `origin:claude-plugin` in `recall --sources`.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("origin:claude-plugin"),
        "recall --sources must label a plugin source with origin:claude-plugin: {}",
        sources.stdout
    );
}

#[test]
fn marketplace_catalog_origin_label_in_recall_sources() {
    // spec: MKT-10
    // A source whose items came from a .claude-plugin/marketplace.json (both the
    // catalog itself and its sub-sources) is labelled `origin:claude-marketplace`.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("origin:claude-marketplace"),
        "recall --sources must label a marketplace source with origin:claude-marketplace: {}",
        sources.stdout
    );
}

#[test]
fn mind_toml_suppresses_plugin_manifest_with_note() {
    // spec: MKT-2
    // When a source has an authoritative mind.toml (one that declares [[items]])
    // AND a .claude-plugin/plugin.json, the mind.toml wins and a note is printed
    // saying the plugin manifest was found but ignored.
    let sb = Sandbox::bare("toml-wins");
    // Authoritative mind.toml declaring one item.
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"toml-skill\"\npath = \"skills/toml-skill/SKILL.md\"\n",
    );
    sb.write_and_commit(
        "skills/toml-skill/SKILL.md",
        "---\nname: toml-skill\ndescription: From mind.toml\n---\n# toml\n",
    );
    // Also has a plugin.json declaring different items.
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"plugin-name","version":"1.0","description":"Plugin desc"}"#,
    );
    sb.write_and_commit(
        "skills/plugin-skill/SKILL.md",
        "---\nname: plugin-skill\ndescription: From plugin\n---\n# plugin\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // Advisory note: mind.toml is authoritative, plugin manifest is ignored.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("authoritative mind.toml")
            || combined.contains(".claude-plugin/ manifest is ignored"),
        "meld must print the advisory note about mind.toml suppressing plugin manifest: {combined}"
    );

    // Items come from mind.toml, not from the plugin.json.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:toml-skill"),
        "mind.toml item must appear in probe: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:plugin-skill"),
        "plugin item must not appear (mind.toml is authoritative): {}",
        probe.stdout
    );
    // The plugin-name prefix from plugin.json must also not be applied.
    assert!(
        !probe.stdout.contains("skill:plugin-name:"),
        "plugin-name prefix must not appear: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_does_not_emit_claude_plugin_manifest() {
    // spec: MKT-11
    // Consuming a marketplace does not make mind a plugin publisher: no
    // .claude-plugin/ directory is created in the store or lobe, and `dump`
    // output contains no .claude-plugin reference.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success);

    // No .claude-plugin dir in the store.
    let store_dir = sb.mind_home.join("store");
    if store_dir.is_dir() {
        for entry in std::fs::read_dir(&store_dir).unwrap().flatten() {
            let p = entry.path();
            assert!(
                !p.join(".claude-plugin").exists(),
                "store entry must not contain .claude-plugin: {:?}",
                p
            );
        }
    }

    // No .claude-plugin dir in the lobe.
    assert!(
        !sb.claude_home.join(".claude-plugin").exists(),
        "lobe must not contain a .claude-plugin dir"
    );

    // `dump` output contains no .claude-plugin reference.
    let dump = sb.mind(&["dump"]);
    assert!(
        !dump.stdout.contains(".claude-plugin"),
        "dump output must contain no .claude-plugin reference: {}",
        dump.stdout
    );
}

#[test]
fn marketplace_entry_version_wins_over_plugin_json_version() {
    // spec: MKT-8
    // A marketplace entry's declared `version` is authoritative over the in-repo
    // plugin.json's own `version` (mirrors Claude's "strict": false). The existing
    // MKT-8 test only proves the entry NAME wins; this proves the VERSION field
    // does too, surfaced via `recall --sources --json`.
    let catalog = Sandbox::bare("mkt8-version");
    // In-repo plugin whose plugin.json declares version 9.9.9.
    catalog.write_and_commit(
        "plugins/myplugin/.claude-plugin/plugin.json",
        r#"{"name":"myplugin","version":"9.9.9"}"#,
    );
    catalog.write_and_commit(
        "plugins/myplugin/skills/theskill/SKILL.md",
        "---\nname: theskill\ndescription: A skill\n---\n# theskill\n",
    );
    // The catalog entry declares a DIFFERENT version 1.1.1 (entry wins).
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"test","plugins":[{"name":"override","source":"./plugins/myplugin","version":"1.1.1"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Under MKT-14 in-repo entries are catalog items of the parent source;
    // the entry-level version is not stored per-sub-source. What IS
    // observable: the skill is discoverable under the entry name as prefix
    // (MKT-8 entry name wins).
    let probe = catalog.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("override:theskill"),
        "skill must appear under entry name as prefix (MKT-8): {}",
        probe.stdout
    );
}

#[test]
fn mind_toml_suppresses_marketplace_manifest_with_note() {
    // spec: MKT-2
    // The marketplace analogue of mind_toml_suppresses_plugin_manifest_with_note:
    // an authoritative mind.toml alongside a .claude-plugin/marketplace.json wins,
    // a note is printed, and NONE of the marketplace's sub-source plugins are
    // melded (only the mind.toml item exists).
    let catalog = Sandbox::bare("mkt2-market-suppress");
    // Authoritative mind.toml declaring one rule.
    catalog.write_and_commit(
        "rules/my-rule.md",
        "---\ndescription: my rule\n---\n# rule\n",
    );
    catalog.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"rule\"\nname = \"my-rule\"\npath = \"rules/my-rule.md\"\n",
    );
    // A marketplace.json that would otherwise meld an in-repo plugin sub-source.
    catalog.write_and_commit(
        "plugins/embedded/.claude-plugin/plugin.json",
        r#"{"name":"embedded","version":"0.1"}"#,
    );
    catalog.write_and_commit(
        "plugins/embedded/skills/embskill/SKILL.md",
        "---\nname: embskill\ndescription: embedded skill\n---\n# embskill\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"embedded","source":"./plugins/embedded"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("authoritative mind.toml")
            || combined.contains(".claude-plugin/ manifest is ignored"),
        "meld must print the advisory note for a suppressed marketplace manifest: {combined}"
    );

    // No sub-source was registered from the suppressed marketplace.
    let sources = catalog.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("embedded"),
        "the marketplace's in-repo plugin must NOT be melded when mind.toml is authoritative: {}",
        sources.stdout
    );

    // Only the mind.toml rule item exists; the plugin skill does not.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:my-rule"),
        "the mind.toml rule must be present: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("embskill"),
        "the suppressed plugin's skill must not appear: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_sync_rewalk_registers_new_entry() {
    // spec: MKT-7 MKT-14
    // After melding a catalog, adding a new in-repo plugin entry to marketplace.json
    // and running `sync` must make that plugin's items discoverable. Under MKT-14
    // in-repo entries are scan roots of the catalog (not sub-melded), so the items
    // appear under the catalog source after sync fetches the updated commit; the new
    // plugin does NOT appear as a separate registered source.
    let catalog = Sandbox::bare("mkt-sync-rewalk");
    // First plugin present from the start.
    catalog.write_and_commit(
        "plugins/first/.claude-plugin/plugin.json",
        r#"{"name":"first"}"#,
    );
    catalog.write_and_commit(
        "plugins/first/skills/oneskill/SKILL.md",
        "---\nname: oneskill\ndescription: skill one\n---\n# one\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"first","source":"./plugins/first"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // Only the catalog source is registered; "second" does not yet exist.
    let before = catalog.mind(&["recall", "--sources"]);
    assert!(
        !before.stdout.contains("second"),
        "second must not exist before it is added: {}",
        before.stdout
    );

    // Add a SECOND in-repo plugin and list it in the marketplace, then commit.
    catalog.write_and_commit(
        "plugins/second/.claude-plugin/plugin.json",
        r#"{"name":"second"}"#,
    );
    catalog.write_and_commit(
        "plugins/second/skills/twoskill/SKILL.md",
        "---\nname: twoskill\ndescription: skill two\n---\n# two\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"first","source":"./plugins/first"},{"name":"second","source":"./plugins/second"}]}"#,
    );

    // Sync must fetch the catalog and pick up the new plugin via the marketplace scan.
    let sync = catalog.mind(&["sync"]);
    assert!(sync.success, "sync failed: {} {}", sync.stdout, sync.stderr);

    // Under MKT-14 in-repo plugins are scan roots of the catalog, not sub-melded sub-sources.
    // "second" must NOT appear as a registered source; its items surface via the catalog scan.
    let after = catalog.mind(&["recall", "--sources"]);
    assert!(
        !after.stdout.contains("/second") && !after.stdout.contains("second "),
        "second must not be a registered sub-source (MKT-14): {}",
        after.stdout
    );
    // The new plugin's skill is discoverable via the catalog's marketplace scan.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("twoskill"),
        "the newly-added plugin's skill must be discoverable after sync: {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Plugin repos / marketplace catalogs as [discover].sources entries (MKT-12/13)
// ---------------------------------------------------------------------------

#[test]
fn nested_source_plugin_inherits_plugin_name_as_namespace() {
    // spec: MKT-12
    // A [discover].sources entry pointing at a repo that carries a
    // .claude-plugin/plugin.json and has no explicit `namespace` uses the
    // plugin's `name` field as the effective prefix for that nested source --
    // exactly as if it had been melded directly under MKT-5.
    let plugin_repo = Sandbox::bare("mkt12-plugin");
    plugin_repo.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Plugin greet skill\n---\n# greet\n",
    );
    plugin_repo.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"myplugin","version":"1.0"}"#,
    );

    let super_src = Sandbox::bare("mkt12-super");
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n",
            plugin_repo.source_spec()
        ),
    );

    let spec = super_src.source_spec();
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The nested plugin source is registered with the plugin name as prefix.
    let probe = super_src.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("myplugin:greet"),
        "nested plugin source must use plugin name as default prefix (MKT-12): {}",
        probe.stdout
    );
}

#[test]
fn nested_source_marketplace_preserves_per_plugin_namespacing() {
    // spec: MKT-13
    // A [discover].sources entry that points at a marketplace catalog preserves
    // per-plugin namespacing: each plugin's `name` field from the catalog entry
    // becomes the effective prefix for that plugin's items (MKT-8), regardless
    // of any outer namespace set on the [discover].sources entry.
    let market_repo = Sandbox::bare("mkt13-market");
    market_repo.write_and_commit(
        "plugins/alpha/.claude-plugin/plugin.json",
        r#"{"name":"alpha"}"#,
    );
    market_repo.write_and_commit(
        "plugins/alpha/skills/search/SKILL.md",
        "---\nname: search\ndescription: Alpha search skill\n---\n# search\n",
    );
    market_repo.write_and_commit(
        "plugins/beta/.claude-plugin/plugin.json",
        r#"{"name":"beta"}"#,
    );
    market_repo.write_and_commit(
        "plugins/beta/skills/search/SKILL.md",
        "---\nname: search\ndescription: Beta search skill\n---\n# search\n",
    );
    market_repo.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"TestMarket","plugins":[{"name":"alpha","source":"./plugins/alpha"},{"name":"beta","source":"./plugins/beta"}]}"#,
    );

    let super_src = Sandbox::bare("mkt13-super");
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n",
            market_repo.source_spec()
        ),
    );

    let spec = super_src.source_spec();
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Both plugins appear under their per-plugin names through the nested chain (MKT-13).
    let probe = super_src.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("alpha:search"),
        "alpha's skill must be namespaced under 'alpha' through the nested chain (MKT-13): {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("beta:search"),
        "beta's skill must be namespaced under 'beta' through the nested chain (MKT-13): {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Cross-source skill collision at meld (NS-43/NS-45)
// ---------------------------------------------------------------------------

#[test]
fn marketplace_same_root_multiple_plugins_with_skills_array() {
    // spec: MKT-14
    // Multiple in-repo plugins all with "source": "./" and explicit skills arrays
    // (the Anthropic pattern). Each plugin's listed skills are scanned under its
    // own entry name as prefix. Skills from different plugins do not cross-contaminate.
    let sb = Sandbox::bare("mkt14-same-root");

    // Plugin 1: document-skills
    sb.write_and_commit(
        "skills/xlsx/SKILL.md",
        "---\nname: xlsx\ndescription: xlsx\n---\n# xlsx\n",
    );
    sb.write_and_commit(
        "skills/docx/SKILL.md",
        "---\nname: docx\ndescription: docx\n---\n# docx\n",
    );

    // Plugin 2: example-skills
    sb.write_and_commit(
        "skills/art/SKILL.md",
        "---\nname: art\ndescription: art\n---\n# art\n",
    );
    sb.write_and_commit(
        "skills/design/SKILL.md",
        "---\nname: design\ndescription: design\n---\n# design\n",
    );

    // Plugin 3: api-skills
    sb.write_and_commit(
        "skills/claude-api/SKILL.md",
        "---\nname: claude-api\ndescription: api\n---\n# api\n",
    );

    // marketplace.json: three plugins, all source: "./", each with a disjoint skills array
    sb.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{
  "name": "anthropic-agent-skills",
  "plugins": [
    {"name": "document-skills", "source": "./", "skills": ["./skills/xlsx", "./skills/docx"]},
    {"name": "example-skills",  "source": "./", "skills": ["./skills/art", "./skills/design"]},
    {"name": "api-skills",      "source": "./", "skills": ["./skills/claude-api"]}
  ]
}"#,
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe", "--no-tui"]);
    // document-skills plugin
    assert!(
        probe.stdout.contains("document-skills:xlsx"),
        "expected document-skills:xlsx: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("document-skills:docx"),
        "expected document-skills:docx: {}",
        probe.stdout
    );
    // example-skills plugin
    assert!(
        probe.stdout.contains("example-skills:art"),
        "expected example-skills:art: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("example-skills:design"),
        "expected example-skills:design: {}",
        probe.stdout
    );
    // api-skills plugin
    assert!(
        probe.stdout.contains("api-skills:claude-api"),
        "expected api-skills:claude-api: {}",
        probe.stdout
    );

    // No skill appears under the wrong prefix (partition is correct).
    assert!(
        !probe.stdout.contains("example-skills:xlsx"),
        "xlsx must not appear under example-skills: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("document-skills:art"),
        "art must not appear under document-skills: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_forget_removes_plugin_installed_item() {
    // spec: MKT-1
    // `forget` uninstalls a plugin-sourced skill: the lobe symlink is removed and
    // `recall` no longer finds the item.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["forget", "skill:acme-tools:greet"]);
    assert!(r.success, "forget must succeed: {} {}", r.stdout, r.stderr);

    // The symlink must be gone.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/acme-tools:greet")).is_err(),
        "symlink must be removed after forget"
    );

    // `recall` must no longer list the item.
    assert!(
        !sb.mind(&["recall", "skill:acme-tools:greet"]).success,
        "recall must fail after forget (item no longer installed)"
    );
}

#[test]
fn marketplace_upgrade_plugin_sourced_item() {
    // spec: MKT-1, MKT-6
    // Upgrading after a source change updates the installed plugin item; the hash
    // advances. Drift is source-content-hash driven (MKT-6), not by declared plugin version.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let before = sb.mind(&["recall", "skill:acme-tools:greet"]).stdout;

    // Update the skill content and commit.
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet (updated)\n",
    );
    assert!(
        sb.mind(&["sync"]).success,
        "sync must succeed after source update"
    );

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        r.success,
        "upgrade --yes must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgraded skill:acme-tools:greet"),
        "upgrade must report the item as upgraded: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:acme-tools:greet"]).stdout;
    assert_ne!(
        before, after,
        "recall output must differ after upgrade (hash must advance)"
    );
}

#[test]
fn marketplace_introspect_fix_relinks_plugin_item() {
    // spec: MKT-1
    // Manually removing a plugin-item symlink and running `introspect --fix`
    // restores it, confirming the plugin install path uses the same registry as
    // convention items.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let link = sb.claude_home.join("skills/acme-tools:greet");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "symlink must exist before the test removes it"
    );
    std::fs::remove_file(&link).unwrap();

    let r = sb.mind(&["introspect", "--fix"]);
    assert!(
        r.success,
        "introspect --fix must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("relinked"),
        "introspect --fix must report the link was recreated: {}",
        r.stdout
    );
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "symlink must be restored by introspect --fix"
    );
    assert!(
        sb.mind(&["introspect"]).stdout.contains("all good"),
        "introspect must be clean after --fix"
    );
}

#[test]
fn marketplace_recall_shows_installed_plugin_item() {
    // spec: MKT-1, MKT-5
    // `recall` (item list) includes an installed plugin item by its effective
    // (prefixed) name. Distinct from the probe assertions elsewhere which only
    // check availability, not the installed-item listing.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let list = sb.mind(&["recall"]);
    assert!(
        list.success,
        "recall must succeed: {} {}",
        list.stdout, list.stderr
    );
    assert!(
        list.stdout.contains("skill:acme-tools:greet"),
        "recall must list the installed plugin skill by its prefixed name: {}",
        list.stdout
    );
}

#[test]
fn marketplace_source_only_mind_toml_composes_with_plugin() {
    // spec: MKT-2
    // A `[source]`-only mind.toml (no `[[items]]` or `[discover]`, so non-authoritative)
    // composes with a .claude-plugin/plugin.json: the mind.toml prefix wins over the
    // plugin name (MKT-5 precedence), and the plugin still supplies items.
    let sb = Sandbox::bare("compose-toml");
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"custom\"\n");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"plugin-name","version":"1.0"}"#,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        r.success,
        "meld --link-only must succeed: {} {}",
        r.stdout, r.stderr
    );

    let probe = sb.mind(&["probe"]);
    // mind.toml prefix wins over plugin name (MKT-5 precedence).
    assert!(
        probe.stdout.contains("skill:custom:greet"),
        "mind.toml prefix must win over plugin name: {}",
        probe.stdout
    );
    // Plugin name must NOT be used as prefix when mind.toml has one.
    assert!(
        !probe.stdout.contains("skill:plugin-name:greet"),
        "plugin name must not be used as prefix when mind.toml supplies one: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_mind_toml_roots_suppress_manifest_own_items_with_note() {
    // spec: MKT-15
    // A repo ships a .claude-plugin/plugin.json AND a mind.toml declaring a
    // [source].roots scan layout. The roots layout is an own-item directive: it
    // suppresses the manifest's own-item layer, so convention discovery under the
    // root supplies the repo's items, the plugin's components are not scanned, and
    // meld prints a note that the manifest's plugin components are ignored.
    let sb = Sandbox::bare("mkt15-roots");
    // The plugin manifest would (absent MKT-15) contribute skills/greet as acme:greet.
    sb.write_and_commit(".claude-plugin/plugin.json", "{\n  \"name\": \"acme\"\n}\n");
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: plugin greet\n---\n# greet\n",
    );
    // The mind.toml relocates convention to pkg/, defining the repo's own items.
    sb.write_and_commit("mind.toml", "[source]\nroots = [\"pkg\"]\n");
    sb.write_and_commit(
        "pkg/skills/own/SKILL.md",
        "---\nname: own\ndescription: convention own skill\n---\n# own\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // The MKT-15 suppression note is printed.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin components are ignored"),
        "meld must note the manifest's own-item layer is ignored: {combined}"
    );

    let probe = sb.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:own"),
        "convention skill under the declared root must be discovered: {probe}"
    );
    assert!(
        !probe.contains("greet"),
        "the plugin manifest's own items must be suppressed by the roots layout: {probe}"
    );
}

#[test]
fn marketplace_consumer_root_flag_suppresses_manifest_with_note() {
    // spec: MKT-15, DSC-51
    // A consumer `meld --root pkg` on a manifest source with no mind.toml scan
    // layout is an own-item directive too: it suppresses the manifest's own-item
    // layer, convention discovery under the consumer root supplies the items, and
    // meld prints the suppression note (naming the flag). Without this, --root
    // would be a silent no-op on a manifest source.
    let sb = Sandbox::bare("mkt15-consumer-root");
    // The plugin manifest would (absent MKT-15) contribute skills/greet as acme:greet.
    sb.write_and_commit(".claude-plugin/plugin.json", "{\n  \"name\": \"acme\"\n}\n");
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: plugin greet\n---\n# greet\n",
    );
    // The repo's own items live under pkg/, reached only via a consumer --root.
    sb.write_and_commit(
        "pkg/skills/own/SKILL.md",
        "---\nname: own\ndescription: convention own skill\n---\n# own\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "pkg", "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin components are ignored")
            && combined.contains("--root/--flat-skills"),
        "meld must note the manifest is ignored and name the consumer flag: {combined}"
    );

    let probe = sb.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:own"),
        "convention skill under the consumer root must be discovered: {probe}"
    );
    assert!(
        !probe.contains("greet"),
        "the plugin manifest's own items must be suppressed by --root: {probe}"
    );
}

#[test]
fn marketplace_and_curator_compose_via_discover_sources() {
    // spec: MKT-16
    // A repo ships a .claude-plugin/marketplace.json (an in-repo plugin) AND a
    // mind.toml whose only discovery content is [discover].sources (a curator
    // directive over OTHER repos). The two compose: the marketplace still defines
    // the immediate source (its in-repo plugin item is discovered, not suppressed),
    // and the curated nested source is registered. No suppression note is printed.
    let curator = Sandbox::bare("curator");
    let nested = Sandbox::named("curated"); // a normal source with fixture items
    // Marketplace with one in-repo plugin: the curator's own items.
    curator.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{ "name": "M", "plugins": [ {"name": "toolkit", "source": "./plugins/toolkit"} ] }"#,
    );
    curator.write_and_commit(
        "plugins/toolkit/skills/format/SKILL.md",
        "---\nname: format\ndescription: fmt\n---\n# format\n",
    );
    // A co-present mind.toml with ONLY [discover].sources (curator directive).
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[source]\ndescription = \"marketplace + curator\"\n\n[discover]\nsources = [{{ source = \"{}\" }}]\n",
            nested.source_spec()
        ),
    );

    let spec = curator.source_spec();
    let r = curator.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // Compose: the marketplace's own in-repo plugin item IS discovered (the
    // [discover].sources list does not suppress the manifest).
    let probe = curator.mind(&["probe"]).stdout;
    assert!(
        probe.contains("toolkit:format"),
        "the marketplace in-repo plugin item must be discovered (not suppressed by [discover].sources): {probe}"
    );
    // And the curated nested source is registered.
    let sources = curator.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("/curated"),
        "the curated nested source must be registered alongside the marketplace: {sources}"
    );
    // No manifest-suppression note: a bare [discover].sources is not an own-item directive.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("manifest's plugin components are ignored")
            && !combined.contains("manifest is ignored"),
        "a [discover].sources-only mind.toml must not suppress the manifest: {combined}"
    );
}

#[test]
fn example_marketplace_curator_validates() {
    // spec: MKT-15, MKT-16
    // The marketplace-curator example is both a Claude marketplace and a mind
    // curator. It validates clean structurally (review does not clone the nested
    // chain), pinning that the compose shape parses and passes author validation.
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/marketplace-curator");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(
        r.success,
        "marketplace-curator example must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn marketplace_plugin_with_only_unsupported_components() {
    // spec: MKT-3, MKT-4, MKT-18
    // A plugin with no skills, agents, or commands (only hooks/, plus a
    // non-markdown file and a nested group under commands/) succeeds on meld,
    // prints a skipped-components note, and contributes zero items to probe.
    // The `.sh` and the subdirectory are the point: the command scan takes flat
    // `<name>.md` files only, so neither is an item -- and neither may be
    // dropped in silence, so both are counted as unmapped commands/ entries.
    let sb = Sandbox::bare("no-items-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"hooks-only","version":"1.0"}"#,
    );
    sb.write_and_commit("hooks/post.sh", "#!/bin/sh\n");
    sb.write_and_commit("commands/do.sh", "#!/bin/sh\n");
    sb.write_and_commit(
        "commands/frontend/component.md",
        "---\ndescription: nested\n---\n# component\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld of a plugin with only unsupported components must succeed: {} {}",
        r.stdout, r.stderr
    );

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("not installed (no mind equivalent)"),
        "meld must print the skipped-components note: {combined}"
    );
    assert!(
        combined.contains("2 unmapped commands/ entries"),
        "the note must count the commands/ entries the flat .md scan left \
         behind, or MKT-4's 'never a silent drop' is false: {combined}"
    );

    // Since this is the only melded source, probe must have no item lines.
    let probe = sb.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:")
            && !probe.stdout.contains("agent:")
            && !probe.stdout.contains("rule:")
            && !probe.stdout.contains("command:")
            && !probe.stdout.contains("tool:"),
        "a hooks-only plugin must contribute zero items to probe: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_with_only_commands_installs_them_without_a_skipped_note() {
    // spec: MKT-18, MKT-4
    // The inverse of the case above: a plugin whose only component is a
    // commands/ directory of markdown now contributes items, and nothing is
    // reported as skipped.
    let sb = Sandbox::bare("commands-only-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"cmds","version":"1.0"}"#,
    );
    sb.write_and_commit(
        "commands/ship.md",
        "---\ndescription: Ship it\n---\n# ship\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("not installed (no mind equivalent)"),
        "a plugin whose commands all install must print no skipped note: {combined}"
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("command:cmds:ship"),
        "the plugin's command must be offered: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_reserved_kind_word_plugin_name_installs_bare() {
    // spec: MKT-5, NS-25
    // When a plugin.json `name` is a reserved kind word (e.g. "skill"), the resilience
    // path in catalog.rs (NS-25 validate_prefix guard) silently falls through to no
    // prefix, so items install bare rather than making the source un-meldable.
    let sb = Sandbox::bare("reserved-name-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"skill","version":"1.0"}"#,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        r.success,
        "meld of a plugin named with a reserved kind word must succeed: {} {}",
        r.stdout, r.stderr
    );

    let probe = sb.mind(&["probe"]);
    // Items install bare (no prefix) when the plugin name is a reserved kind word.
    assert!(
        probe.stdout.contains("skill:greet"),
        "item must install bare when plugin name is a reserved kind word: {}",
        probe.stdout
    );
    // The reserved word must NOT be used as a prefix (would produce "skill:skill:greet").
    assert!(
        !probe.stdout.contains("skill:skill:greet"),
        "reserved kind word must not be used as prefix: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_external_plugin_learnable_on_demand() {
    // spec: MKT-7, DSC-54
    // An external marketplace plugin registered by a plain meld (without --recursive)
    // is available (discoverable via probe) and learnable on demand via `learn`,
    // even though it was not auto-installed at meld time.
    let extplugin = Sandbox::bare("extplugin-ondemand");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );

    let catalog = Sandbox::bare("ext-catalog-ondemand");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"cat","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );

    let cat_spec = catalog.source_spec();
    // Plain meld (no --yes, no --recursive): external plugin registers but is not installed.
    let r = catalog.mind(&["meld", &cat_spec]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // External skill must NOT be in the lobe yet.
    assert!(
        !catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin skill must not be installed after plain meld"
    );

    // External skill IS discoverable via probe.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:ext:extskill"),
        "external plugin skill must be discoverable after plain meld: {}",
        probe.stdout
    );

    // On-demand learn must install it.
    let learn = catalog.mind(&["learn", "ext:extskill"]);
    assert!(
        learn.success,
        "learn of external plugin skill must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin skill must be linked after learn"
    );
}

#[test]
fn marketplace_dump_of_plugin_source_contains_no_claude_plugin_ref() {
    // spec: MKT-11, MKT-6
    // `dump` of a source that came from a plugin manifest:
    // (a) succeeds and produces output;
    // (b) contains NO ".claude-plugin" reference (MKT-11: mind does not emit manifests);
    // (c) contains the source spec so a re-meld would work.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "meld --yes must succeed"
    );

    let dump = sb.mind(&["dump"]);
    assert!(
        dump.success,
        "dump must succeed: {} {}",
        dump.stdout, dump.stderr
    );
    assert!(
        !dump.stdout.is_empty(),
        "dump output must be non-empty for a plugin source"
    );
    assert!(
        !dump.stdout.contains(".claude-plugin"),
        "dump must not contain any .claude-plugin reference (MKT-11): {}",
        dump.stdout
    );
}

#[test]
fn marketplace_upgrade_catalog_sub_source_item() {
    // spec: MKT-7, MKT-6
    // Upgrading a catalog sub-source item after the sub-source content changes:
    // `sync` re-walks the marketplace and detects the update; `upgrade --yes` applies it.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    // Plain meld (no --yes) to skip auto-install; then learn the item explicitly.
    assert!(sb.mind(&["meld", &spec]).success, "meld must succeed");
    let learn = sb.mind(&["learn", "alpha:one"]);
    assert!(
        learn.success,
        "learn alpha:one must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let before = sb.mind(&["recall", "skill:alpha:one"]).stdout;

    // Update the sub-source item and commit.
    sb.write_and_commit(
        "plugins/alpha/skills/one/SKILL.md",
        "---\nname: one\ndescription: Alpha skill (updated)\n---\n# one updated\n",
    );
    assert!(
        sb.mind(&["sync"]).success,
        "sync must succeed after sub-source update"
    );

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        r.success,
        "upgrade --yes must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgraded skill:alpha:one"),
        "upgrade must report the sub-source item as upgraded: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:alpha:one"]).stdout;
    assert_ne!(
        before, after,
        "recall output must differ after upgrade (hash must advance)"
    );
}

// ---------------------------------------------------------------------------
// init-source --marketplace / --flat-skills / --namespace (INIT-10..12)
// ---------------------------------------------------------------------------
