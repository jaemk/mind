//! Source discovery and scanning (spec/discovery.md).

use super::*;

/// A block-scalar description with a whitespace-only line of multi-byte
/// whitespace once panicked the frontmatter dedent, aborting every verb that
/// scans the source. `meld` and `probe` must instead read it.
#[test]
fn multibyte_whitespace_in_a_block_description_does_not_abort_the_scan() {
    // spec: DSC-22
    let sb = Sandbox::bare("ideographic-space");
    sb.write_and_commit(
        "skills/wide/SKILL.md",
        "---\nname: wide\ndescription: |-\n  first\n \u{3000}\n  second\n---\n# wide\n",
    );
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("wide"), "{}", probe.stdout);
    assert!(probe.stdout.contains("first"), "{}", probe.stdout);
}

/// A SKILL.md `description:` carrying an ANSI color escape and a bidi-override
/// code point (U+202E) must reach every display surface sanitized: raw ESC and
/// the bidi override must not survive to `probe --no-tui`, `recall`, or
/// `--json` stdout, closing the terminal-injection gap for the frontmatter
/// path (the plugin-manifest path already sanitized at store time).
#[test]
fn hostile_frontmatter_description_is_sanitized_everywhere() {
    // spec: DSC-94
    let sb = Sandbox::bare("hostile-desc");
    sb.write_and_commit(
        "skills/evil/SKILL.md",
        "---\nname: evil\ndescription: \"\x1b[31mRED\x1b[0m \u{202E}oot now\"\n---\n# evil\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // probe --no-tui (human listing).
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        !probe.stdout.contains('\x1b') && !probe.stdout.contains('\u{202E}'),
        "probe --no-tui must not leak raw ANSI/bidi: {:?}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("RED") && probe.stdout.contains("oot now"),
        "the surrounding text must still be shown: {}",
        probe.stdout
    );

    // probe --json.
    let probe_json = sb.mind(&["probe", "--json"]);
    assert!(probe_json.success, "{}", probe_json.stderr);
    assert!(
        !probe_json.stdout.contains('\x1b') && !probe_json.stdout.contains('\u{202E}'),
        "probe --json must not leak raw ANSI/bidi (serde only escapes ANSI, \
         not the bidi override): {:?}",
        probe_json.stdout
    );

    // recall <item> (human).
    assert!(sb.mind(&["learn", "skill:evil"]).success);
    let recall = sb.mind(&["recall", "skill:evil"]);
    assert!(recall.success, "{}", recall.stderr);
    assert!(
        !recall.stdout.contains('\x1b') && !recall.stdout.contains('\u{202E}'),
        "recall must not leak raw ANSI/bidi: {:?}",
        recall.stdout
    );

    // recall <item> --json.
    let recall_json = sb.mind(&["recall", "skill:evil", "--json"]);
    assert!(recall_json.success, "{}", recall_json.stderr);
    assert!(
        !recall_json.stdout.contains('\x1b') && !recall_json.stdout.contains('\u{202E}'),
        "recall --json must not leak raw ANSI/bidi: {:?}",
        recall_json.stdout
    );
}

/// A `mind.toml` `[source].description` carrying the same hostile content must
/// be sanitized too (the mind.toml variant of B4/DSC-94), including through
/// `sync`'s re-read.
#[test]
fn hostile_mind_toml_source_description_is_sanitized_everywhere() {
    // spec: DSC-94
    let sb = Sandbox::bare("hostile-source-desc");
    // TOML basic strings require control/bidi bytes to be written as `\uXXXX`
    // escapes, not embedded raw; the parsed value still carries the real
    // ESC/U+202E code points, so this exercises the same sanitization path.
    sb.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"\\u001b[31mRED\\u001b[0m \\u202Eoot now\"\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success, "{}", sources.stderr);
    assert!(
        !sources.stdout.contains('\x1b') && !sources.stdout.contains('\u{202E}'),
        "recall --sources must not leak raw ANSI/bidi from [source].description: {:?}",
        sources.stdout
    );

    let sources_json = sb.mind(&["recall", "--sources", "--json"]);
    assert!(sources_json.success, "{}", sources_json.stderr);
    assert!(
        !sources_json.stdout.contains('\x1b') && !sources_json.stdout.contains('\u{202E}'),
        "recall --sources --json must not leak raw ANSI/bidi: {:?}",
        sources_json.stdout
    );

    // Re-synced (re-read) description stays sanitized too.
    sb.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"\\u001b[32mGREEN\\u001b[0m \\u202Eoot again\"\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let after_sync = sb.mind(&["recall", "--sources"]);
    assert!(
        !after_sync.stdout.contains('\x1b') && !after_sync.stdout.contains('\u{202E}'),
        "a re-synced [source].description must stay sanitized: {:?}",
        after_sync.stdout
    );
}

/// DSC-95/DSC-96/CLI-232: an item NAME is source-controlled and keys identity.
/// DSC-96 (a sibling fix landed concurrently with this one) now REJECTS a
/// hostile name (a control byte, an ANSI escape, or a bidi/zero-width code
/// point) at catalog-scan time, so a freshly melded source can no longer
/// install one -- see `hostile_named_item_is_skipped_at_scan_not_installed`
/// below, which pins that gate. That closes the vector for a NEW install, but
/// a manifest entry with a hostile name is still a live, reachable state: an
/// item installed by an older binary (before DSC-96 existed), or a hand-edited
/// `manifest.json`. `recall`, `introspect`, and `forget` all read the manifest
/// directly and must still sanitize such an entry for display, exactly as
/// DSC-95 requires. This test injects a hostile manifest entry the way a
/// pre-DSC-96 install would have left one, then drives those three surfaces.
///
/// A hostile-named UNMANAGED lobe item is exercised separately below
/// (`hostile_unmanaged_item_name_is_skipped_via_unm9`): unlike a hand-edited
/// manifest entry, an unmanaged item's name is derived fresh from its filename
/// on every scan, and UNM-9 (the unmanaged-lobe analog of DSC-96) now skips a
/// hostile derived name at scan time (with a warning) rather than letting it
/// through to UNM-5's sanitize-for-disclosure step, so that test pins a
/// not-found ref rather than a disclosure.
///
/// Every assertion below is a PAIR: absence of the raw escape/bidi bytes, AND
/// presence of the sanitized name ("review"/"handmade" -- both chosen so the
/// escape/bidi bytes sit strictly inside a run of letters that survives
/// sanitizing intact). Checking absence alone would also pass a regression
/// that dropped the hostile item from the output entirely, or replaced it
/// with a placeholder.
#[test]
fn hostile_installed_item_is_sanitized_at_recall_introspect_and_forget() {
    // spec: DSC-95
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Hand-edit manifest.json: clone the "review" entry under a hostile
    // `kind:name` key, with an on-disk name carrying an ANSI CSI color
    // sequence and a bidi override (U+202E). "review" stays intact inside the
    // hostile bytes so the sanitized-but-present checks work. The bare_name no
    // longer matches any real catalog item, so this also doubles as an
    // "installed but removed upstream" (orphan) row for `recall`/`introspect`.
    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null(), "the review entry must exist: {doc}");
    // A C0 control byte (BEL, not part of a `[`-bracketed CSI sequence) plus a
    // bidi override. Deliberately avoids '[' (and '*'/'?'): `resolve::is_glob`
    // treats a literal '[' as a glob metacharacter, which would misroute
    // `forget`'s ref through its glob-selection path instead of the exact-match
    // path this test means to exercise -- an unrelated ref-parsing quirk, not a
    // DSC-95 sanitization gap (CSI/SGR-sequence stripping is already covered
    // elsewhere, e.g. `hostile_frontmatter_description_is_sanitized_everywhere`).
    let hostile_name = format!("re{}vie\u{202E}w", '\x07');
    let hostile_key = format!("skill:{hostile_name}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String(hostile_name.clone());
    hostile["bare_name"] = serde_json::Value::String(hostile_name.clone());
    // Store/link paths must be distinct so `forget` does not try to remove
    // the real "review" install's files.
    hostile["store"] = serde_json::Value::String(format!("store/skill/{hostile_name}"));
    hostile["links"] = serde_json::Value::Array(vec![]);
    doc["items"][&hostile_key] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    // recall: the injected item is now an orphan row (installed, no catalog
    // match under its hostile bare_name).
    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "{}", recall.stderr);
    assert!(
        !recall.stdout.contains('\x1b') && !recall.stdout.contains('\u{202E}'),
        "recall must not leak raw ANSI/bidi from an installed item name: {:?}",
        recall.stdout
    );
    assert!(
        recall.stdout.contains("skill:review"),
        "recall must still show the sanitized key (twice: the real item and \
         the orphan row): {:?}",
        recall.stdout
    );

    let recall_json = sb.mind(&["recall", "--json"]);
    assert!(recall_json.success, "{}", recall_json.stderr);
    assert!(
        !recall_json.stdout.contains('\x1b') && !recall_json.stdout.contains('\u{202E}'),
        "recall --json must not leak raw ANSI/bidi: {:?}",
        recall_json.stdout
    );
    assert!(
        recall_json.stdout.contains("skill:review"),
        "recall --json must still show the sanitized key: {:?}",
        recall_json.stdout
    );

    // Single-item recall (human + json), resolved by the raw hostile key typed
    // as the ref (DSC-95: identity is the raw name; only display sanitizes).
    for args in [
        vec!["recall", hostile_key.as_str()],
        vec!["recall", hostile_key.as_str(), "--json"],
    ] {
        let out = sb.mind(&args);
        assert!(out.success, "{args:?}: {}", out.stderr);
        assert!(
            !out.stdout.contains('\x1b') && !out.stdout.contains('\u{202E}'),
            "{args:?} must not leak raw ANSI/bidi: {:?}",
            out.stdout
        );
        assert!(
            out.stdout.contains("review"),
            "{args:?} must still show the sanitized name: {:?}",
            out.stdout
        );
    }

    // introspect: the injected item's bare_name matches no catalog item, so it
    // is reported `removed-upstream`, naming the sanitized key in its message.
    let introspect = sb.mind(&["introspect"]);
    assert!(
        !introspect.stdout.contains('\x1b') && !introspect.stdout.contains('\u{202E}'),
        "introspect must not leak raw ANSI/bidi: {:?}",
        introspect.stdout
    );
    assert!(
        introspect.stdout.contains("skill:review"),
        "introspect must still show the sanitized key: {:?}",
        introspect.stdout
    );
    assert!(
        introspect.stdout.contains("no longer present"),
        "introspect must still report the removed-upstream finding: {:?}",
        introspect.stdout
    );

    let introspect_json = sb.mind(&["introspect", "--json"]);
    assert!(introspect_json.success, "{}", introspect_json.stderr);
    assert!(
        !introspect_json.stdout.contains('\x1b') && !introspect_json.stdout.contains('\u{202E}'),
        "introspect --json must not leak raw ANSI/bidi: {:?}",
        introspect_json.stdout
    );
    let introspect_v = parse_json(&introspect_json.stdout);
    let found = introspect_v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["target"] == "skill:review" && i["kind"] == "removed-upstream");
    assert!(
        found,
        "introspect --json must report the sanitized target: {introspect_v}"
    );

    // `--json` `removed` array (forget), built from the sanitized key. Note:
    // this checks the `removed[]` entries specifically, not the whole
    // document -- the envelope's `target` field echoes back the raw ref the
    // user themselves typed on the command line (not source-controlled data
    // newly introduced by this document), so DSC-95 does not apply to it.
    let forget_json = sb.mind(&["forget", &hostile_key, "--json", "--yes"]);
    assert!(forget_json.success, "{}", forget_json.stderr);
    let forget_v = parse_json(&forget_json.stdout);
    assert_eq!(
        forget_v["removed"].as_array().map(|a| a.len()),
        Some(1),
        "forget --json must record the removed item: {forget_v}"
    );
    assert_eq!(
        forget_v["removed"][0], "skill:review",
        "the removed[] entry must be the sanitized key: {forget_v}"
    );

    // The real "review" install is untouched: it was a distinct manifest key.
    assert!(sb.claude_home.join("skills/review/SKILL.md").exists());
}

/// `recall --tree`'s two rendering paths (`InstalledGraph::render_installed_node`
/// / `build_node`, human and `--json`) also read installed names straight off
/// the manifest, independent of the `recall`/`introspect`/`forget` sites the
/// sibling test above covers. This drives them through the "installed but not
/// in the dependency graph" fallback (no catalog match under the hostile
/// bare_name, so `render_subtree`/`subtree_node` return `None`), the one path
/// that builds its own display line rather than delegating to
/// `InstalledItem::display_key()` elsewhere.
#[test]
fn hostile_installed_item_name_is_sanitized_in_recall_tree() {
    // spec: DSC-95
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null());
    let hostile_name = format!("re{}vie\u{202E}w", '\x07');
    let hostile_key = format!("skill:{hostile_name}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String(hostile_name.clone());
    hostile["bare_name"] = serde_json::Value::String(hostile_name.clone());
    hostile["store"] = serde_json::Value::String(format!("store/skill/{hostile_name}"));
    hostile["links"] = serde_json::Value::Array(vec![]);
    doc["items"][&hostile_key] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let tree = sb.mind(&["recall", &hostile_key, "--tree"]);
    assert!(tree.success, "{}", tree.stderr);
    assert!(
        !tree.stdout.contains('\x07') && !tree.stdout.contains('\u{202E}'),
        "recall --tree must not leak raw control/bidi bytes: {:?}",
        tree.stdout
    );
    assert!(
        tree.stdout.contains("review"),
        "recall --tree must still show the sanitized name: {:?}",
        tree.stdout
    );

    let tree_json = sb.mind(&["recall", &hostile_key, "--tree", "--json"]);
    assert!(tree_json.success, "{}", tree_json.stderr);
    assert!(
        !tree_json.stdout.contains('\x07') && !tree_json.stdout.contains('\u{202E}'),
        "recall --tree --json must not leak raw control/bidi bytes: {:?}",
        tree_json.stdout
    );
    let v = parse_json(&tree_json.stdout);
    assert_eq!(
        v["key"], "skill:review",
        "the --tree --json node's key must be the sanitized key: {v}"
    );
}

/// DSC-96: a source shipping an item whose on-disk name carries a control
/// byte, an ANSI escape, or a bidi/zero-width code point is skipped at
/// catalog-scan time (with a warning naming the source), rather than being
/// tolerated and merely sanitized for display. Pins the gate that
/// `hostile_installed_item_is_sanitized_at_recall_introspect_and_forget`
/// above depends on to explain why it injects a manifest entry by hand
/// instead of driving `meld`/`learn` end-to-end.
#[test]
fn hostile_named_item_is_skipped_at_scan_not_installed() {
    // spec: DSC-96
    let sb = Sandbox::bare("hostile-name-scan");
    let name = format!("re{}[31mvie\u{202E}w", '\x1b');
    sb.write_and_commit(
        &format!("skills/{name}/SKILL.md"),
        "---\ndescription: a skill\n---\n# skill\n",
    );
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(
        meld.success,
        "meld itself must still succeed: {}",
        meld.stderr
    );
    assert!(
        meld.stderr.contains("unsafe item name"),
        "meld must warn that the hostile-named item was skipped: {}",
        meld.stderr
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("skill:review"),
        "the hostile-named item must not appear in the catalog at all: {}",
        recall.stdout
    );
}

/// M5/DSC-95: a `mind.toml` `[discover]` glob is source-controlled and is echoed
/// back verbatim in a rejection message (absolute glob, `..`, or an
/// out-of-root/unsafe-name confinement error).
///
/// Asserting only "no raw ANSI/bidi in stderr" is not enough on its own: an
/// earlier form of this test passed only because its fixture used the old flat
/// `[discover].skills` array, which is now a TOML type error caught before
/// DSC-81's check ever runs. So this pins the actual DSC-81 rejection (an
/// absolute glob) alongside the sanitizing, using the current
/// `skills = { include = [...] }` schema that reaches it.
#[test]
fn hostile_discover_glob_error_is_sanitized_and_names_the_dsc81_rejection() {
    // spec: DSC-95
    let sb = Sandbox::bare("hostile-glob");
    // An absolute glob (rejected by DSC-81) whose text carries ESC + U+202E.
    // TOML basic strings encode the control/bidi bytes as `\uXXXX` escapes; the
    // parsed value still carries the real code points.
    sb.write_and_commit(
        "mind.toml",
        "[discover]\nskills = { include = [\"/abs\\u001b[31mINJ\\u001b[0m\\u202E/*/SKILL.md\"] }\n",
    );
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(
        !meld.success,
        "an absolute discover glob must be rejected (DSC-81)"
    );
    // M-test2: absence-of-escapes alone would also pass a TOML parse error, an
    // unrelated meld failure, or a message that dropped the offending pattern
    // entirely. Name the actual DSC-81 rejection (an absolute path is not
    // allowed as a discover glob) so this pins the real cause.
    assert!(
        meld.stderr.contains("absolute") || meld.stderr.contains("must be relative"),
        "the failure must specifically name the absolute-glob rejection, not \
         some other meld failure: {}",
        meld.stderr
    );

    // Confirm via `--json`: the structured error kind must be the DSC-81
    // rejection (a `mind.toml` schema/content error), and its message must
    // still name the absolute-glob cause, not some other/generic failure.
    let meld_json = sb.mind(&["meld", &sb.source_spec(), "--json"]);
    assert!(!meld_json.success);
    let v = parse_json(&meld_json.stdout);
    assert_eq!(
        v["error"]["kind"], "mind-toml",
        "expected the mind-toml error kind: {v}"
    );
    let message = v["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("absolute") || message.contains("must be relative"),
        "the --json error message must also name the absolute-glob rejection: {v}"
    );

    // DSC-95: the source-controlled pattern is echoed stripped, in both the
    // human message and the `--json` document. serde escapes ESC but not a bidi
    // override, so the raw character would otherwise survive the envelope.
    assert!(
        !meld.stderr.contains('\x1b') && !meld.stderr.contains('\u{202E}'),
        "the rejected glob must be echoed stripped on stderr: {:?}",
        meld.stderr
    );
    assert!(
        !message.contains('\x1b') && !message.contains('\u{202E}'),
        "the rejected glob must be echoed stripped in --json: {message:?}"
    );
    // The pattern is still named, not dropped.
    assert!(
        message.contains("INJ"),
        "the sanitized pattern must still be shown: {message:?}"
    );
}

#[test]
fn a_stray_item_manifest_drops_its_item_not_every_melded_source() {
    // spec: DSC-98
    // An item directory's `mind.toml` is read on every catalog walk, so a
    // source that ships one mind cannot read must not take `probe`/`recall`
    // down for every OTHER melded source, the way a hard scan failure would.
    let sb = melded();
    let stray = Sandbox::bare("stray-item-manifest");
    stray.write_and_commit(
        "skills/vendored/SKILL.md",
        "---\nname: vendored\ndescription: a vendored sub-source\n---\n# vendored\n",
    );
    // A whole source manifest copied into a skill directory: `[source]` and
    // `[[items]]` are outside the item schema, whose only table is `[[hooks]]`.
    stray.write_and_commit(
        "skills/vendored/mind.toml",
        "[source]\ndescription = \"a source manifest in an item dir\"\n",
    );
    stray.write_and_commit(
        "skills/healthy/SKILL.md",
        "---\nname: healthy\ndescription: a normal sibling\n---\n# healthy\n",
    );
    let meld = sb.mind(&["meld", &stray.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "one unreadable item manifest must not fail the meld: {} {}",
        meld.stdout, meld.stderr
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.success,
        "probe must still run: {} {}",
        probe.stdout, probe.stderr
    );
    assert!(
        probe.stdout.contains("skill:review"),
        "the other source's items must still be listed: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:healthy"),
        "the stray source's other items must still be listed: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("vendored"),
        "the item with the unreadable manifest must not be offered: {}",
        probe.stdout
    );
    // The drop is not silent, and the warning names the file to open and the
    // rule it broke.
    assert!(
        probe.stderr.contains("vendored") && probe.stderr.contains("mind.toml"),
        "the warning must name the item and its manifest: {}",
        probe.stderr
    );
    assert!(
        probe.stderr.contains("[[hooks]]"),
        "the warning must name the one-table rule (HOOK-131): {}",
        probe.stderr
    );

    // recall is the other read-only verb that walks every source.
    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall must still run: {}", recall.stderr);

    // And the hook-executing path is not blocked for anything else: the
    // dropped item is simply unresolvable, its sibling installs normally.
    let learn_dropped = sb.mind(&["learn", "vendored", "--yes"]);
    assert!(
        !learn_dropped.success,
        "a dropped item must not be installable: {}",
        learn_dropped.stdout
    );
    let learn_ok = sb.mind(&["learn", "healthy", "--yes"]);
    assert!(
        learn_ok.success,
        "a healthy sibling must still install: {} {}",
        learn_ok.stdout, learn_ok.stderr
    );
}

#[test]
fn hostile_item_manifest_parse_error_is_sanitized() {
    // spec: DSC-95 DSC-98
    // A raw ESC byte in a TOML file is itself a parse error, so the `toml`
    // crate's annotated snippet (which quotes the offending line verbatim) is
    // the guaranteed path for that byte to reach a terminal. With DSC-98 that
    // path runs on `probe`, a verb the user did not point at any source.
    let sb = melded();
    let hostile = Sandbox::bare("hostile-item-manifest");
    hostile.write_and_commit(
        "skills/trap/SKILL.md",
        "---\nname: trap\ndescription: trap\n---\n# trap\n",
    );
    hostile.write_and_commit(
        "skills/trap/mind.toml",
        "\u{1b}[31mINJ\u{1b}[0m\u{202E} = broken\n",
    );
    assert!(
        sb.mind(&["meld", &hostile.source_spec(), "--register-only"])
            .success
    );

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "probe must still run: {}", probe.stderr);
    assert!(
        !probe.stderr.contains('\x1b') && !probe.stderr.contains('\u{202E}'),
        "the quoted input line must be echoed stripped: {:?}",
        probe.stderr
    );
    // Stripped, not dropped: the text that identifies the offending line is
    // still there.
    assert!(
        probe.stderr.contains("INJ"),
        "the sanitized snippet must still be shown: {:?}",
        probe.stderr
    );
    assert!(
        !probe.stdout.contains("trap"),
        "the item must be dropped: {}",
        probe.stdout
    );
}

#[test]
fn hostile_root_mind_toml_parse_error_is_sanitized() {
    // spec: DSC-95
    // The root manifest's parse error quotes the offending input line the same
    // way an item manifest's does, and it is reached on the meld of any source
    // whose `mind.toml` does not parse.
    let sb = Sandbox::bare("hostile-root-manifest");
    sb.write_and_commit("mind.toml", "\u{1b}[31mINJ\u{1b}[0m\u{202E} = broken\n");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(!meld.success, "an unparsable mind.toml must be refused");
    assert!(
        meld.stderr.contains("invalid mind.toml"),
        "the failure must name the file it could not parse: {}",
        meld.stderr
    );
    assert!(
        !meld.stderr.contains('\x1b') && !meld.stderr.contains('\u{202E}'),
        "the quoted input line must be echoed stripped: {:?}",
        meld.stderr
    );
    assert!(
        meld.stderr.contains("INJ"),
        "the sanitized snippet must still be shown: {:?}",
        meld.stderr
    );
}

#[test]
fn an_unprefixed_source_shipping_a_colon_name_is_warned_about() {
    // spec: DSC-99
    // `:` is legal in an item name (CMD-2/CMD-6 recommend it for a command
    // group) but illegal in a prefix (NS-72), so an unprefixed source can spell
    // a name that is indistinguishable from a prefixed source's item. Naming it
    // at scan is the only detection available.
    let sb = Sandbox::bare("forged-namespace");
    sb.write_and_commit(
        "commands/acme:deploy.md",
        "---\ndescription: deploy the thing\n---\n# deploy\n",
    );
    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(meld.success, "the item is still offered: {}", meld.stderr);
    assert!(
        meld.stderr.contains("acme:deploy") && meld.stderr.contains("no namespace prefix"),
        "meld must warn that an unprefixed source spelled a namespace: {}",
        meld.stderr
    );

    let learn = sb.mind(&["learn", "command:acme:deploy", "--dry-run"]);
    assert!(learn.success, "{}", learn.stderr);
    assert!(
        learn.stderr.contains("no namespace prefix"),
        "learn must warn over the closure it would install: {}",
        learn.stderr
    );

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("command:acme:deploy"),
        "the name installs as written, warning or not: {}",
        probe.stdout
    );
    // spec: DSC-101 -- the scan never writes the warning: `probe` re-scans on
    // its ~1s TUI poll tick, so a scan-time stderr line would repeat forever
    // and land in the alternate screen.
    assert!(
        !probe.stderr.contains("no namespace prefix"),
        "a read-only surface must not warn: {}",
        probe.stderr
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stderr.contains("no namespace prefix"),
        "a read-only surface must not warn: {}",
        recall.stderr
    );

    // A source that IS prefixed produces the same shape legitimately, and must
    // not be warned about.
    let prefixed = Sandbox::bare("real-namespace");
    prefixed.write_and_commit("mind.toml", "[source]\nprefix = \"acme\"\n");
    prefixed.write_and_commit(
        "commands/deploy.md",
        "---\ndescription: deploy the thing\n---\n# deploy\n",
    );
    let sb2 = Sandbox::bare("consumer");
    assert!(
        sb2.mind(&["meld", &prefixed.source_spec(), "--register-only"])
            .success
    );
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("command:acme:deploy"),
        "the prefixed source produces the same effective name: {}",
        probe2.stdout
    );
    assert!(
        !probe2.stderr.contains("no namespace prefix"),
        "a real prefix must not be warned about: {}",
        probe2.stderr
    );
}

#[test]
fn dsc93_does_not_soften_a_top_level_meld_of_a_missing_sources_tree_path() {
    // spec: DSC-93 CLI-213
    // DSC-93's predicate is "a non-existent path inside mind's own sources
    // tree", which is a shape a user can also type at the top level -- pasting
    // a clone path out of a `recall`/`introspect` message, say. The skip is
    // deliberately scoped to `[discover].sources` entries, where the path was
    // MISRESOLVED by DSC-92 rather than chosen. At the top level the same path
    // must still be the hard error CLI-213 promises, not a warning: the user
    // named this source, so "it isn't there" is the answer they asked for, and
    // a warn-and-continue would exit 0 having registered nothing.
    let sb = Sandbox::bare("dsc93-toplevel");
    let inside = sb
        .mind_home
        .join("sources/local/someone/never-cloned")
        .to_string_lossy()
        .into_owned();
    let r = sb.mind(&["meld", &inside]);
    assert!(
        !r.success,
        "a top-level meld of a missing path inside the sources tree must still \
         hard-fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not a directory"),
        "the failure must name the missing path: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("resolves inside mind's own sources tree"),
        "the DSC-93 skip rationale must not be applied to a path the user named \
         directly: {}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("never-cloned"),
        "nothing may be registered: {}",
        sources.stdout
    );
}

#[test]
fn meld_resolves_a_curated_relative_nested_source_against_the_curator_not_the_cwd() {
    // spec: DSC-92 -- the only coverage was a direct `MindToml::load` unit
    // test; nothing proved the rewrite reaches the real `meld` walk. The
    // curator declares `source = "../explicit"` (the shape
    // examples/super-source ships) and the meld runs from a cwd that is NOT the
    // curator's parent, so a cwd-relative resolution finds nothing and the
    // nested source is silently never registered.
    let sb = Sandbox::bare("curator");
    nested_repo(&sb.base.join("explicit"), "greet");
    sb.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"../explicit\"\n",
    );

    // Stand somewhere whose PARENT is not the curator's parent, so a
    // cwd-relative reading of `../explicit` cannot accidentally land on the
    // right directory. (A cwd of `<base>/anything` would: `../explicit` from
    // there is `<base>/explicit` by coincidence.)
    let elsewhere = sb.base.join("far/away");
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert!(
        !elsewhere.join("../explicit").is_dir(),
        "the decoy cwd must not resolve '../explicit' to anything"
    );
    let spec = sb.source_spec();
    let r = sb.mind_cwd(&["meld", &spec, "--register-only"], &elsewhere);
    assert!(
        r.success,
        "melding the curator must succeed: {} {}",
        r.stdout, r.stderr
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("explicit"),
        "the relative nested source must be registered even though the consumer stood elsewhere: \
         {}\nmeld stderr: {}",
        sources.stdout,
        r.stderr
    );
    // ...and it is the curator's real sibling, so its item is discoverable.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("greet"),
        "the nested source's item must be offered: {}",
        recall.stdout
    );
}

#[test]
fn sync_rewalk_resolves_a_newly_curated_relative_nested_source_against_the_curator() {
    // spec: DSC-92 DSC-57 -- the second of the three call sites the statement
    // says the read-site fix covers "with no per-caller change needed": a
    // nested entry added to the curator AFTER the initial meld is picked up by
    // `sync`'s re-walk, and must resolve against the curator too.
    let sb = Sandbox::bare("curator-sync");
    nested_repo(&sb.base.join("first"), "alpha");
    sb.write_and_commit("mind.toml", "[[discover.sources]]\nsource = \"../first\"\n");

    // See the meld test above: the cwd's parent must not be the curator's
    // parent, or a cwd-relative reading of `../second` would land correctly by
    // coincidence and the test would pass with the DSC-92 rewrite removed.
    let elsewhere = sb.base.join("far/away");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let spec = sb.source_spec();
    assert!(
        sb.mind_cwd(&["meld", &spec, "--register-only"], &elsewhere)
            .success
    );

    // Curator gains a second relative nested source.
    nested_repo(&sb.base.join("second"), "beta");
    sb.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"../first\"\n[[discover.sources]]\nsource = \"../second\"\n",
    );

    let s = sb.mind_cwd(&["sync"], &elsewhere);
    assert!(s.success, "sync must succeed: {} {}", s.stdout, s.stderr);

    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("second"),
        "sync's re-walk must register the newly curated relative nested source: {sources}\n\
         sync stdout: {}",
        s.stdout
    );
}

#[test]
fn init_source_oversized_pre_existing_mind_toml_is_refused() {
    // spec: DSC-91 -- `init-source` refuses a pre-existing mind.toml over the
    // metadata cap, end to end. Proven with a lowered cap rather than an 8 MiB
    // fixture: a 1-byte cap must refuse even a tiny mind.toml. This does NOT
    // isolate init-source's own pre-read (the catalog scan's load of the same
    // file would refuse it too); that read is pinned by the unit test
    // `commands::tests::init_source_pre_read_of_mind_toml_is_capped`.
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring");
    write(
        &repo.join("mind.toml"),
        "[source]\ndescription = \"small\"\n",
    );
    let dir = repo.to_str().unwrap();

    let r = sb.mind(&["init-source", dir, "--max-metadata-size", "1"]);
    assert!(
        !r.success,
        "a 1-byte cap must refuse init-source's read of an existing mind.toml: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind.toml"),
        "error must name mind.toml: {combined}"
    );
}

#[test]
fn probe_surfaces_frontmatter_descriptions() {
    // spec: DSC-2, DSC-20
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success);
    assert!(
        r.stdout.contains("Review the diff for bugs"),
        "expected skill description in probe output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Implements a spec with tests"),
        "{}",
        r.stdout
    );
}

#[test]
fn mind_toml_is_authoritative_and_overrides_link_and_description() {
    // spec: DSC-3, DSC-32, STO-2
    let sb = Sandbox::new();
    // A rule in a non-conventional location, declared explicitly with a custom
    // link target and description override.
    sb.write_and_commit(
        "guidelines/style.md",
        "---\ndescription: from frontmatter\n---\n# house style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"a test library\"\n\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"guidelines/style.md\"\n",
            "link = \"rules/custom-style.md\"\n",
            "description = \"override wins\"\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    // Authoritative: only the declared item is visible; convention dirs are not scanned.
    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("rule:style"), "{}", probe.stdout);
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);
    // Description override beats frontmatter.
    assert!(probe.stdout.contains("override wins"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("from frontmatter"),
        "{}",
        probe.stdout
    );

    // [source].description surfaces in `recall --sources`.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("a test library"),
        "{}",
        sources.stdout
    );

    // Custom link target is honored.
    assert!(sb.mind(&["learn", "style"]).success);
    let link = sb.claude_home.join("rules/custom-style.md");
    let meta = std::fs::symlink_metadata(&link).expect("custom link should exist");
    assert!(meta.file_type().is_symlink());
}

#[test]
fn mind_toml_discover_globs_find_items() {
    // spec: DSC-33, DSC-3
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: a glob-found skill\n---\n# foo\n",
    );
    sb.write_and_commit(
        "mind.toml",
        "[discover]\nskills = { include = [\"packages/*/SKILL.md\"] }\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:foo"), "{}", probe.stdout);
    assert!(
        probe.stdout.contains("a glob-found skill"),
        "{}",
        probe.stdout
    );
    // Convention scanning is off, so the conventional skill is absent.
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);
}

#[test]
fn mind_toml_discover_exclude_drops_matches() {
    // spec: DSC-37
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: foo\n---\n# foo\n",
    );
    sb.write_and_commit(
        "packages/internal-x/SKILL.md",
        "---\ndescription: internal\n---\n# internal\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[discover.skills]\n",
            "include = [\"packages/*/SKILL.md\"]\n",
            "exclude = [\"packages/internal-*/SKILL.md\"]\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:foo"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("skill:internal-x"),
        "{}",
        probe.stdout
    );
}

#[test]
fn super_source_recursively_melds_listed_sources() {
    // spec: DSC-38, CLI-15
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);

    // The curated source's items are available...
    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:review"), "{}", probe.stdout);
    // ...and both sources are registered (the curated one tracks its own upstream).
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(sources.stdout.contains("tools"), "{}", sources.stdout);
    assert!(sources.stdout.contains("registry"), "{}", sources.stdout);
    assert!(registry.mind(&["learn", "review"]).success);
}

#[test]
fn super_source_applies_nested_alias() {
    // spec: DSC-39
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", as = \"tl\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    assert!(registry.mind(&["meld", &spec]).success);

    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:tl:review"), "{}", probe.stdout);
}

#[test]
fn on_auth_failure_field_accepted_in_super_source() {
    // spec: DSC-68 -- on-auth-failure is a valid field on a nested source entry.
    // No auth failure occurs here (the nested source is a reachable local repo),
    // so the field is simply parsed: the meld must succeed and register the
    // nested source, proving deny_unknown_fields accepts the schema.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", on-auth-failure = {{ action = \"skip\", message = \"Configure credentials: https://example.com/auth\" }} }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld of a super-source with on-auth-failure must succeed: {}",
        r.stderr
    );
    // The nested source melded normally: its items are available and it is
    // registered (no auth failure, so the policy never fires).
    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:review"), "{}", probe.stdout);
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(sources.stdout.contains("tools"), "{}", sources.stdout);
}

#[test]
fn on_auth_failure_invalid_action_rejected_in_super_source() {
    // spec: DSC-68 -- an on-auth-failure action that is neither "error" nor
    // "skip" is rejected by serde at mind.toml parse time (AuthFailureAction is
    // a typed enum); the error surfaces as a TOML parse failure at meld time.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", on-auth-failure = {{ action = \"warn\" }} }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "an invalid on-auth-failure action must fail the meld"
    );
    assert!(
        r.stderr.contains("on-auth-failure") || r.stderr.contains("expected 'error' or 'skip'"),
        "error must explain the invalid action: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_skip_continues_meld() {
    // spec: DSC-68, DSC-69
    // A super-source with a nested entry whose clone fails with an auth error and
    // action = "skip": the meld exits zero, the nested source is not registered,
    // a warning is printed to stderr.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Non-JSON mode: meld succeeds; nested source not registered; warning on stderr.
    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        r.success,
        "skip action must not abort meld: stderr={}",
        r.stderr
    );
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("private-repo"),
        "skipped source must not be registered: {}",
        sources.stdout
    );
    assert!(
        r.stderr.contains("unable to meld source") && r.stderr.contains("skipping"),
        "auth failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_skip_json_output_has_skipped_array() {
    // spec: DSC-68, DSC-69
    // Under --json, a skipped nested source appears in a "skipped" array on the
    // single root object (one JSON output, not multiple).
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(
        r.success,
        "skip action --json must exit zero: stderr={}",
        r.stderr
    );
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "action field: {}", r.stdout);
    assert_eq!(v["outcome"], "melded", "outcome field: {}", r.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(skipped.len(), 1, "exactly one skipped entry: {}", r.stdout);
    assert_eq!(
        skipped[0]["reason"], "auth_failure",
        "reason must be auth_failure: {}",
        r.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("private-repo"),
        "source must name the skipped repo: {}",
        r.stdout
    );
    // Warning goes to stderr even under --json.
    assert!(
        r.stderr.contains("unable to meld source"),
        "warning must appear on stderr under --json too: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_error_fails_meld() {
    // spec: DSC-68, DSC-69
    // action = "error": when the nested source clone fails with auth error, meld
    // exits non-zero.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "error action must cause non-zero exit: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn on_auth_failure_error_prints_message() {
    // spec: DSC-68, DSC-69
    // action = "error" with message: the message and the standard auth-failure
    // line appear on stderr, in both plain and --json modes.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\", message = \"Configure credentials.\" }\n",
    );
    let spec = registry.source_spec();

    // Plain mode.
    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(!r.success, "error action must fail: {}", r.stderr);
    assert!(
        r.stderr.contains("unable to meld source"),
        "standard auth-failure line must appear: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("Configure credentials."),
        "curator message must appear on stderr: {}",
        r.stderr
    );

    // JSON mode: the auth-failure warning lines (printed by the meld command
    // before returning the error, DSC-69) still appear on stderr regardless of
    // --json. The final MindError goes to stdout as the CLI-181 envelope.
    let rj = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(!rj.success, "error action --json must fail: {}", rj.stderr);
    assert!(
        rj.stderr.contains("unable to meld source"),
        "standard auth-failure line must appear under --json: {}",
        rj.stderr
    );
    assert!(
        rj.stderr.contains("Configure credentials."),
        "curator message must appear on stderr under --json: {}",
        rj.stderr
    );
}

#[test]
fn on_auth_failure_error_json_emits_error_envelope() {
    // spec: DSC-68, DSC-69, CLI-181
    // action = "error" with --json: the process exits non-zero and emits the
    // JSON error envelope on stdout. The auth-failure warning is printed to
    // stderr by the meld command before the error is returned (DSC-69), and
    // the final MindError is wrapped in the CLI-181 envelope on stdout.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "error action --json must exit non-zero: {}",
        r.stderr
    );
    // The final MindError goes to stdout as a JSON error envelope (CLI-181).
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert!(
        v["error"]["kind"].is_string(),
        "error envelope must have a kind: {}",
        r.stdout
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "error envelope must have a non-empty message: {}",
        r.stdout
    );
    // The auth-failure warning (printed before the error is returned) still
    // appears on stderr (DSC-69: always warn to stderr regardless of --json).
    assert!(
        r.stderr.contains("unable to meld source"),
        "auth-failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_absent_propagates_as_generic_error() {
    // spec: DSC-68
    // When a nested source fails auth and has NO on-auth-failure config, the
    // error propagates as a generic git error (hard failure, non-zero exit).
    // This is distinct from action = "error": no standardized DSC-69 message,
    // just the raw git output.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    // No on-auth-failure field at all.
    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/unconfigured-private\"\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "auth failure without on-auth-failure must fail: {}",
        r.stderr
    );
    // The failure is the raw git error, NOT the structured DSC-69 message.
    assert!(
        !r.stderr.contains("unable to meld source"),
        "no structured auth-failure message without on-auth-failure config: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_multiple_nested_sources_all_skipped() {
    // spec: DSC-68, DSC-69
    // A super-source with two nested entries that both fail auth with
    // action = "skip": both are skipped, meld exits zero, and under --json
    // the skipped array has two entries.  Uses separate sandbox instances for
    // plain and JSON modes because a successful meld registers the source and
    // a second meld of the same source would hit SourceExists.
    let toml_body = "[[discover.sources]]\nsource = \"https://example.com/owner/private-one\"\non-auth-failure = { action = \"skip\" }\n\n[[discover.sources]]\nsource = \"https://example.com/owner/private-two\"\non-auth-failure = { action = \"skip\" }\n";

    // Plain mode: exit zero, two auth-failure warnings on stderr.
    {
        let registry = Sandbox::bare("registry");
        let fake_dir = fake_git_bin_dir(&registry.base);
        let new_path = prepend_path(&fake_dir);
        registry.write_and_commit("mind.toml", toml_body);
        let spec = registry.source_spec();
        let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
        assert!(
            r.success,
            "two skipped nested sources must not abort meld: {}",
            r.stderr
        );
        assert_eq!(
            r.stderr.matches("unable to meld source").count(),
            2,
            "one warning per skipped source: {}",
            r.stderr
        );
    }

    // JSON mode: skipped array has exactly two entries, each with reason = "auth_failure".
    {
        let registry = Sandbox::bare("registry");
        let fake_dir = fake_git_bin_dir(&registry.base);
        let new_path = prepend_path(&fake_dir);
        registry.write_and_commit("mind.toml", toml_body);
        let spec = registry.source_spec();
        let rj = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
        assert!(rj.success, "meld --json must succeed: {}", rj.stderr);
        let v = parse_json(&rj.stdout);
        let skipped = v["skipped"].as_array().expect("skipped must be an array");
        assert_eq!(
            skipped.len(),
            2,
            "two skipped sources must produce two skipped entries: {}",
            rj.stdout
        );
        for entry in skipped {
            assert_eq!(
                entry["reason"], "auth_failure",
                "each entry must have reason auth_failure: {}",
                rj.stdout
            );
        }
    }
}

#[test]
fn on_auth_failure_skip_during_sync_rewalk() {
    // spec: DSC-68, DSC-69
    // During sync's DSC-57 re-walk, a nested source that was previously skipped
    // (not registered) with action = "skip" is encountered again.  The sync
    // must complete successfully, not fail hard.  This exercises the bug path
    // where the sync re-walk loop did not carry on_auth_failure and let errors
    // propagate via `?`.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Initial meld: registry is registered; private-repo is skipped (auth fail).
    let rm = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    let sources_after_meld = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources_after_meld.contains("private-repo"),
        "private-repo must not be registered after skipped initial meld: {}",
        sources_after_meld
    );

    // sync: re-walks registry's mind.toml, finds private-repo not registered,
    // tries to meld it, hits auth failure again.  With on-auth-failure = skip,
    // sync must complete rather than failing hard.
    let rs = registry.mind_env(&["sync"], &[("PATH", &new_path)]);
    assert!(
        rs.success,
        "sync must complete when nested auth-failure has action=skip: stderr={}",
        rs.stderr
    );
    // The nested source is still not registered (skip, not absorbed).
    let sources_after_sync = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources_after_sync.contains("private-repo"),
        "skipped source must remain unregistered after sync: {}",
        sources_after_sync
    );
    // The warning still appears on stderr.
    assert!(
        rs.stderr.contains("unable to meld source") && rs.stderr.contains("skipping"),
        "auth-failure skip warning must appear on stderr during sync: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_skip_sync_json_has_skipped_array() {
    // spec: DSC-68, DSC-69
    // Under sync --json, a skipped nested source (auth-fail, action=skip)
    // discovered during the re-walk appears in the "skipped" array of the
    // sync result object.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Initial meld (non-JSON): establishes registry; private-repo is skipped.
    let rm = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);

    // sync --json: skipped entry from the re-walk appears in the result.
    let rs = registry.mind_env(&["sync", "--json"], &[("PATH", &new_path)]);
    assert!(
        rs.success,
        "sync --json must exit zero with skip: {}",
        rs.stderr
    );
    let v = parse_json(&rs.stdout);
    assert_eq!(v["action"], "sync", "action field: {}", rs.stdout);
    assert_eq!(v["outcome"], "synced", "outcome field: {}", rs.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "one skipped entry in sync result: {}",
        rs.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "auth_failure",
        "reason must be auth_failure: {}",
        rs.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("private-repo"),
        "source must name the skipped repo: {}",
        rs.stdout
    );
    // Warning still on stderr under --json.
    assert!(
        rs.stderr.contains("unable to meld source"),
        "auth-failure warning must appear on stderr under sync --json: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates() {
    // spec: DSC-70
    // T declares A with on-auth-failure=skip; A declares B (no on-auth-failure).
    // When B fails auth, the failure must propagate as a hard error from T's meld;
    // T's skip policy for A must not fire because A itself cloned successfully.
    let a = Sandbox::bare("source_a");
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);

    let r = t.mind_env(&["meld", &t_spec], &[("PATH", &new_path)]);

    // B's auth failure propagates as a hard error; A's skip policy in T must not suppress it.
    assert!(
        !r.success,
        "B's auth failure must exit non-zero, not be absorbed by A's on-auth-failure: stderr={}",
        r.stderr
    );
    // The DSC-69 "unable to meld source" line must not appear: that would mean T
    // misattributed B's failure to A and incorrectly applied A's skip policy.
    assert!(
        !r.stderr.contains("unable to meld source"),
        "B's failure must not be reported as A's auth failure: {}",
        r.stderr
    );
    // A is not persisted: the meld failed before registry.save() was reached.
    let sources = t.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("source_a"),
        "A must not remain registered after the failed meld: {}",
        sources.stdout
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates_during_sync() {
    // spec: DSC-70
    // The same scoping that prevents T's skip-on-A from absorbing B's auth
    // failure at meld time must also hold during sync's re-walk.
    //
    // Setup: T declares A with on-auth-failure=skip. A initially has no
    // mind.toml (so the first meld of T succeeds cleanly). After meld, A is
    // updated to declare B via https:// with no on-auth-failure. The next sync
    // re-walks A's mind.toml, finds B unregistered, attempts to clone B via
    // https://, and hits the fake-git auth failure. Because A is already in the
    // registry (it cloned OK), the code knows the failure came from a descendant
    // and must propagate it hard -- T's skip policy for A must not absorb it.
    let a = Sandbox::bare("source_a");
    // A starts without mind.toml so the initial meld has no nested sources to clone.
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    // Initial meld without fake git: A is a local path with no nested sources;
    // T and A are both registered successfully.
    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    let sources_after_meld = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources_after_meld.contains("source_a"),
        "A must be registered after meld: {}",
        sources_after_meld
    );

    // Now update A to declare B via https:// with no on-auth-failure.
    // Because A is a linked source (local path, no pin), sync reads its live
    // working tree, so this change is picked up without re-melding.
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    // Sync with the fake git binary: when the re-walk finds B (https://) unregistered
    // and tries to clone it, the fake git exits 128 (auth failure).
    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);

    let rs = t.mind_env(&["sync"], &[("PATH", &new_path)]);

    // B's auth failure must propagate as a hard error; T's skip for A must not
    // absorb it because A itself is already in the registry (it cloned OK).
    assert!(
        !rs.success,
        "B's auth failure during sync re-walk must exit non-zero: stderr={}",
        rs.stderr
    );
    // The "unable to meld source" line must NOT appear: that line is only
    // printed when on-auth-failure is present, which it is not for B's entry.
    // Its presence would indicate the sync incorrectly applied T's skip for A
    // to B's failure.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be attributed to A via the DSC-69 message: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates_during_sync_with_error_action() {
    // spec: DSC-70
    // Same descendant-scoping as on_auth_failure_descendant_failure_propagates_during_sync,
    // but T declares A with action = "error" (with a distinctive message) instead
    // of "skip". B (A's descendant, no policy) failing auth during the sync
    // re-walk must propagate hard -- and crucially A's "error" policy must not
    // fire, because A itself cloned fine. If A's policy were misapplied to B, the
    // DSC-69 lines (including A's message) would be printed; they must not be.
    let a = Sandbox::bare("source_a");
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"error\", message = \"A-LEVEL-CREDS-HINT\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    // Initial meld without fake git: A is a local path with no nested sources.
    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    assert!(
        t.mind(&["recall", "--sources"]).stdout.contains("source_a"),
        "A must be registered after meld"
    );

    // A now declares B via https:// with no on-auth-failure of its own.
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);
    let rs = t.mind_env(&["sync"], &[("PATH", &new_path)]);

    // B's auth failure propagates as a hard error even though A's policy is
    // "error" rather than "skip": the outcome is non-zero either way, but the
    // failure must travel via B's generic error path, not A's policy.
    assert!(
        !rs.success,
        "B's auth failure during sync re-walk must exit non-zero with A's error action: stderr={}",
        rs.stderr
    );
    // A's policy must not fire for B: neither the DSC-69 line nor A's message.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be reported via A's DSC-69 auth-failure line: {}",
        rs.stderr
    );
    assert!(
        !rs.stderr.contains("A-LEVEL-CREDS-HINT"),
        "A's curator message must not be printed for B's descendant failure: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_during_sync_json_is_well_formed() {
    // spec: DSC-70
    // Under sync --json, a descendant (B) auth failure that must propagate hard
    // (it is not absorbed by A's skip policy) still exits non-zero, and the JSON
    // channel stays well-formed: no partial success object is emitted on stdout,
    // mirroring the meld error --json contract (stdout empty, diagnostics on
    // stderr).
    let a = Sandbox::bare("source_a");
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);

    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);
    let rs = t.mind_env(&["sync", "--json"], &[("PATH", &new_path)]);

    assert!(
        !rs.success,
        "descendant failure under sync --json must exit non-zero: stderr={}",
        rs.stderr
    );
    // The error path must not emit a partial JSON success object on stdout; if
    // anything is emitted it must still be valid JSON.
    let out = rs.stdout.trim();
    if !out.is_empty() {
        let _: serde_json::Value =
            serde_json::from_str(out).expect("any sync --json stdout must be well-formed JSON");
    }
    // B's failure must not be misattributed to A's skip policy.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be reported via A's DSC-69 auth-failure line: {}",
        rs.stderr
    );
}

// The path fed to a nested entry to force an immediate non-auth clone failure:
// git treats a non-existent local path as a clone error and exits non-zero at
// once, with no network and no auth prompt.
const UNREACHABLE_SOURCE: &str = "/nonexistent/mind-test-source-does-not-exist";

#[test]
fn meld_nested_clone_failure_degrades_gracefully() {
    // spec: DSC-79
    // A super-source with its own items plus one reachable and one unreachable
    // nested entry: the meld succeeds (exit 0), the reachable nested source is
    // registered, the unreachable one is absent, and a warning is on stderr.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::named("super");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a non-auth nested clone failure must not abort the meld: stderr={}",
        r.stderr
    );

    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("tools"),
        "the reachable nested source must remain registered: {sources}"
    );
    assert!(
        sources.contains("super"),
        "the primary super-source must remain registered: {sources}"
    );
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "the unreachable nested source must not be registered: {sources}"
    );
    assert!(
        r.stderr.contains("skipping") && r.stderr.contains("clone"),
        "a clone-failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn meld_nested_clone_failure_json_skipped() {
    // spec: DSC-79
    // Under --json, the skipped nested entry appears in the outer result's
    // `skipped[]` array with `reason == "clone_failure"`.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::named("super");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec, "--json"]);
    assert!(r.success, "meld --json must exit zero: stderr={}", r.stderr);
    let v = parse_json(&r.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "exactly one skipped entry expected: {}",
        r.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "clone_failure",
        "reason must be clone_failure: {}",
        r.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("mind-test-source-does-not-exist"),
        "the skipped entry must name the unreachable source: {}",
        r.stdout
    );
}

#[test]
fn meld_curator_all_nested_fail_hard_fails() {
    // spec: DSC-80
    // A curator-only super-source (no items of its own) whose every nested entry
    // fails to register produces no discoverable items, so the meld hard-fails.
    let super_src = Sandbox::bare("curator");
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "a curator with zero own items and all nested sources failing must hard-fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn meld_curator_one_nested_succeeds_passes() {
    // spec: DSC-80
    // A curator-only super-source with at least one nested source that registers
    // succeeds even when another nested entry fails to clone.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::bare("curator");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a curator with one nested source registered must succeed: stderr={}",
        r.stderr
    );
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("curator"),
        "the curator must be registered: {sources}"
    );
    assert!(
        sources.contains("tools"),
        "the reachable nested source must be registered: {sources}"
    );
}

#[test]
fn meld_nested_clone_failure_descendant_propagates() {
    // spec: DSC-79
    // A super-source T declares A (a reachable curator that clones OK). A declares
    // only B (unreachable). B's non-auth clone failure leaves A a curator with no
    // discoverable items, so A's meld hard-fails (DSC-80). Because A itself is
    // already in the registry, that error came from a descendant and must
    // propagate as a hard failure -- it must NOT be silently swallowed as a
    // clone_failure skip of A by T (the DSC-70 scoping applied to DSC-79).
    let a = Sandbox::bare("source_a");
    a.write_and_commit(
        "mind.toml",
        &format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n"),
    );
    let a_spec = a.source_spec();

    // T has its own items so it is not itself a curator-empty case; the only
    // failure in play originates from A's descendant.
    let t = Sandbox::named("super_t");
    t.write_and_commit(
        "mind.toml",
        &format!("[[discover.sources]]\nsource = \"{a_spec}\"\n"),
    );
    let t_spec = t.source_spec();

    let r = t.mind(&["meld", &t_spec]);
    assert!(
        !r.success,
        "A's descendant failure must propagate as a hard error, not be absorbed as a skip: stderr={}",
        r.stderr
    );
    // A must not be reported as a skipped clone_failure of T: the error is A's own
    // hard failure, arriving after A was already registered.
    assert!(
        !r.stderr.contains("skipping 'source_a'"),
        "A's failure must not be misattributed as a clone_failure skip of A: {}",
        r.stderr
    );
    let sources = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("source_a"),
        "A must not remain registered after the failed meld: {sources}"
    );
}

#[test]
fn meld_curator_all_nested_fail_error_message() {
    // spec: DSC-80
    // The hard-fail produced by a curator-only source with all nested clone
    // failures must carry the CuratorAllNestedFailed message text, not just
    // any non-zero exit.  A different error (e.g. a bad mind.toml parse)
    // would also produce non-zero exit, so this test pins the exact message.
    let super_src = Sandbox::bare("curator-msg");
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "curator with all nested failing must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("produced no discoverable items")
            || r.stderr.contains("no items of its own"),
        "CuratorAllNestedFailed error message must appear on stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("curator-msg"),
        "error message must name the super-source: {}",
        r.stderr
    );
}

#[test]
fn meld_non_curator_all_nested_fail_still_succeeds() {
    // spec: DSC-80
    // DSC-80 fires only when the primary has ZERO items of its own.  A source
    // with at least one own item must succeed even when every nested source
    // fails to clone (the curator-empty guard does not apply).
    let super_src = Sandbox::named("non-curator");
    // Add a mind.toml pointing only to an unreachable nested source.
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a source with own items must succeed even when all nested sources fail: stderr={}",
        r.stderr
    );
    // The primary source must still be registered.
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("non-curator"),
        "primary source must be registered: {sources}"
    );
    // The unreachable source must not be registered.
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "the unreachable nested source must not be registered: {sources}"
    );
    // A clone-failure warning must still be emitted.
    assert!(
        r.stderr.contains("skipping") && r.stderr.contains("clone"),
        "a clone-failure warning must appear even for a non-curator: {}",
        r.stderr
    );
}

#[test]
fn sync_nested_clone_failure_is_skipped() {
    // spec: DSC-79
    // The same non-auth clone-failure skip that applies during meld also applies
    // during sync's DSC-57 re-walk.  Scenario: a super-source is melded without
    // nested sources, then its mind.toml is updated to add an unreachable one.
    // sync re-walks, discovers the new entry, fails to clone it, and must exit
    // zero with a warning rather than hard-failing.
    let super_src = Sandbox::named("rewalk-host");
    let spec = super_src.source_spec();

    // Initial meld: no nested sources, just the super-source itself.
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "initial meld must succeed: {}", r.stderr);

    // Add an unreachable nested source to the super-source's mind.toml.
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);

    // sync re-walks: encounters the new (unreachable) entry, skips it.
    let rs = super_src.mind(&["sync"]);
    assert!(
        rs.success,
        "sync must exit zero when a newly-listed nested source fails to clone: stderr={}",
        rs.stderr
    );
    // The warning must appear.
    assert!(
        rs.stderr.contains("skipping") && rs.stderr.contains("clone"),
        "a clone-failure warning must appear on stderr during sync re-walk: {}",
        rs.stderr
    );
    // The unreachable source must not have been registered.
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "unreachable source must not be registered after sync re-walk: {sources}"
    );
}

#[test]
fn sync_nested_clone_failure_json_has_skipped_entry() {
    // spec: DSC-79
    // Under sync --json, a non-auth clone failure discovered during the
    // DSC-57 re-walk must appear in the result's skipped[] array with
    // reason = "clone_failure".
    let super_src = Sandbox::named("rewalk-json");
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "initial meld must succeed: {}", r.stderr);

    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);

    let rs = super_src.mind(&["sync", "--json"]);
    assert!(
        rs.success,
        "sync --json must exit zero on nested clone failure during re-walk: stderr={}",
        rs.stderr
    );
    let v = parse_json(&rs.stdout);
    assert_eq!(v["action"], "sync", "action field: {}", rs.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "exactly one skipped entry expected in sync --json result: {}",
        rs.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "clone_failure",
        "reason must be clone_failure: {}",
        rs.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("mind-test-source-does-not-exist"),
        "skipped entry must name the unreachable source: {}",
        rs.stdout
    );
}

#[test]
fn super_source_meld_is_cycle_safe() {
    // spec: DSC-38
    // aa and bb each list the other; melding aa must terminate.
    let a = Sandbox::bare("aa");
    let b = Sandbox::bare("bb");
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    b.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            a.source_spec()
        ),
    );
    let spec = a.source_spec();
    let r = a.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);
}

#[test]
fn super_source_meld_breaks_multi_level_cycle() {
    // spec: DSC-38
    // A multi-level chain that loops: aa -> bb -> cc -> aa. Each repo is itself a
    // super-source, so resolution must follow the chain, detect the cycle back to
    // aa, and process each source exactly once (no infinite recursion, no dupes).
    let a = Sandbox::bare("aa");
    let b = Sandbox::bare("bb");
    let c = Sandbox::bare("cc");
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    b.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            c.source_spec()
        ),
    );
    c.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            a.source_spec()
        ),
    );
    let spec = a.source_spec();
    let r = a.mind(&["meld", &spec]);
    assert!(r.success, "a cyclic chain must terminate: {}", r.stderr);
    // Each source is melded exactly once: the per-source "melding" progress line
    // appears three times, not more (a missed cycle guard would loop or repeat).
    assert_eq!(
        r.stdout.matches("melding").count(),
        3,
        "each source melds exactly once: {}",
        r.stdout
    );
    // All three are registered, each exactly once (no duplicate push).
    let recall = a.mind(&["recall", "--sources", "--json"]).stdout;
    for name in ["aa", "bb", "cc"] {
        assert_eq!(
            recall.matches(&format!("\"repo\": \"{name}\"")).count(),
            1,
            "{name} must be registered exactly once: {recall}"
        );
    }
}

#[test]
fn super_source_meld_does_not_auto_install_nested_items() {
    // spec: DSC-54
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);
    // The nested source is registered and its items are available...
    assert!(
        registry.mind(&["probe"]).stdout.contains("skill:review"),
        "the curated source's items must be available"
    );
    // ...but NOT auto-installed: no link is created for a nested item by default.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "a curated super-source must not auto-install the nested chain's items"
    );
    // The user can still install it explicitly.
    assert!(registry.mind(&["learn", "review"]).success);
    assert!(registry.claude_home.join("skills/review").exists());
}

#[test]
fn meld_recursive_installs_nested_items() {
    // spec: DSC-55
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // With --recursive (and --yes to skip prompts), the nested chain's items install.
    let r = registry.mind(&["meld", &spec, "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "the nested source's items must install with --recursive"
    );
}

#[test]
fn meld_recursive_short_flag_installs_nested_items() {
    // spec: DSC-55 - the `-r` short form is equivalent to `--recursive`.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "-r", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "-r must install the nested source's items"
    );
}

#[test]
fn remeld_recursive_installs_nested_chain() {
    // spec: DSC-55
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // First meld without the flag: chain registered, nested items not installed.
    assert!(registry.mind(&["meld", &spec]).success);
    assert!(!registry.claude_home.join("skills/review").exists());
    // Re-melding the already-registered super-source with the flag installs the
    // curated chain's items (nothing is re-registered).
    let r = registry.mind(&["meld", &spec, "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "a re-meld must honor --recursive"
    );
}

#[test]
fn meld_installs_curator_flagged_nested_source_without_recursive() {
    // spec: DSC-58 - a `[discover].sources` entry marked `install = true` has its
    // items offered for install on a plain meld (no --recursive). A sibling entry
    // without the flag is registered but its items are left available.
    let want = Sandbox::bare("want"); // curator recommends installing this one
    want.write_and_commit(
        "skills/want-skill/SKILL.md",
        "---\nname: want-skill\ndescription: wanted\n---\n# want\n",
    );
    let skip = Sandbox::bare("skip"); // registered only, not installed
    skip.write_and_commit(
        "skills/skip-skill/SKILL.md",
        "---\nname: skip-skill\ndescription: skipped\n---\n# skip\n",
    );
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", install = true }}, {{ source = \"{}\" }}]\n",
            want.source_spec(),
            skip.source_spec()
        ),
    );
    // Plain meld, no --recursive. --yes auto-confirms the flagged source's prompt.
    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // Both nested sources are registered.
    let sources = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("/want") && sources.contains("/skip"),
        "both nested sources should be registered: {sources}"
    );
    // The flagged source's item installed; the unflagged source's item did not.
    assert!(
        registry.claude_home.join("skills/want-skill").exists(),
        "the install=true nested source's item must be installed"
    );
    assert!(
        !registry.claude_home.join("skills/skip-skill").exists(),
        "the unflagged nested source's item must not be auto-installed"
    );
}

#[test]
fn meld_recursive_installs_even_unflagged_nested_sources() {
    // spec: DSC-55 DSC-58 - --recursive is the superset: it installs every nested
    // source, including ones the curator did not mark `install = true`.
    let want = Sandbox::bare("want");
    want.write_and_commit(
        "skills/want-skill/SKILL.md",
        "---\nname: want-skill\ndescription: wanted\n---\n# want\n",
    );
    let skip = Sandbox::bare("skip");
    skip.write_and_commit(
        "skills/skip-skill/SKILL.md",
        "---\nname: skip-skill\ndescription: skipped\n---\n# skip\n",
    );
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", install = true }}, {{ source = \"{}\" }}]\n",
            want.source_spec(),
            skip.source_spec()
        ),
    );
    let r = registry.mind(&["meld", &registry.source_spec(), "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/want-skill").exists()
            && registry.claude_home.join("skills/skip-skill").exists(),
        "--recursive installs every nested source regardless of the install flag"
    );
}

#[test]
fn meld_super_source_suggests_probe() {
    // spec: DSC-56
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("mind probe"),
        "melding a curated super-source should suggest probe: {}",
        r.stdout
    );
    // A plain source (no [discover].sources) does not get the hint.
    let plain = Sandbox::named("plain");
    let r2 = plain.mind(&["meld", &plain.source_spec()]);
    assert!(
        !r2.stdout.contains("mind probe"),
        "a normal source must not get the probe hint: {}",
        r2.stdout
    );
}

#[test]
fn sync_rewalks_super_source_for_new_nested_sources() {
    // spec: DSC-57
    let a = Sandbox::bare("aa"); // the curated super-source
    let b = Sandbox::named("bb"); // initially curated
    let c = Sandbox::named("cc"); // added to the list later
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    let spec = a.source_spec();
    assert!(a.mind(&["meld", &spec]).success);
    // Match the `/bb` path segment, not a bare `bb`: a short commit hash is hex,
    // so the two-letter source names (all valid hex) can appear inside it and
    // false-match a bare `contains` (a flaky failure when a hash holds "cc").
    let before = a.mind(&["recall", "--sources"]).stdout;
    assert!(before.contains("/bb"), "{before}");
    assert!(!before.contains("/cc"), "cc not yet listed: {before}");

    // Add cc to aa's discover list.
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}, {{ source = \"{}\" }}]\n",
            b.source_spec(),
            c.source_spec()
        ),
    );
    // sync re-walks aa's [discover].sources and registers the newly listed cc.
    let r = a.mind(&["sync"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        a.mind(&["recall", "--sources"]).stdout.contains("/cc"),
        "sync must register the newly-listed nested source"
    );
}

#[test]
fn invalid_mind_toml_errors_clearly() {
    // spec: DSC-31
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success);
    assert!(r.stderr.contains("unknown item kind"), "{}", r.stderr);
}

#[test]
fn mind_toml_rejects_unknown_fields() {
    // spec: DSC-30
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nbogus = \"x\"\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success);
    assert!(r.stderr.contains("invalid mind.toml"), "{}", r.stderr);
}

#[test]
fn mind_toml_unions_items_and_discover() {
    // spec: DSC-34
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: foo\n---\n# foo\n",
    );
    sb.write_and_commit(
        "extra/special.md",
        "---\nname: special\ndescription: x\n---\n# special\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"agent\"\n",
            "name = \"special\"\n",
            "path = \"extra/special.md\"\n\n",
            "[discover]\n",
            "skills = { include = [\"packages/*/SKILL.md\"] }\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]).stdout;
    assert!(probe.contains("agent:special"), "from [[items]]: {probe}");
    assert!(probe.contains("skill:foo"), "from [discover]: {probe}");
}

#[test]
fn meld_rejects_source_requiring_a_newer_mind() {
    // spec: DSC-40, DSC-116 -- no terminal to ask on and no --yes: refuse.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"9.0\"\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(!r.success, "should refuse a too-new source");
    assert!(r.stderr.contains("requires mind >= 9.0"), "{}", r.stderr);
    assert!(r.stderr.contains("mind evolve"), "{}", r.stderr);
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
    // Rejected: the source is not registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );

    // A satisfiable floor melds fine.
    let ok = Sandbox::new();
    ok.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"0.0.1\"\n");
    let r = ok.mind(&["meld", &ok.source_spec()]);
    assert!(r.success, "{}", r.stderr);
    assert!(!r.stderr.contains("requires mind"), "{}", r.stderr);
}

#[test]
fn meld_yes_melds_a_source_requiring_a_newer_mind_with_a_warning() {
    // spec: DSC-116 -- --yes carries on past the version question, warning.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"9.0\"\n");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("may not work as expected"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("mind evolve"), "{}", r.stderr);
    // One warning for the meld, not one per scan.
    assert_eq!(
        r.stderr.matches("requires mind >= 9.0").count(),
        1,
        "{}",
        r.stderr
    );
    assert!(sb.claude_home.join("skills/review").exists());

    // spec: DSC-40 -- later verbs scan the source with a warning, not an error.
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("review"), "{}", probe.stdout);
    assert!(
        probe.stderr.contains("requires mind >= 9.0"),
        "{}",
        probe.stderr
    );
}

#[test]
fn meld_asks_before_melding_a_source_requiring_a_newer_mind() {
    // spec: DSC-116 -- on a terminal, warn and ask; the default is no.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"9.0\"\n");
    let spec = sb.source_spec();

    let no = sb.run(&["meld", &spec], Some("\n"), &[("MIND_TTY", "1")]);
    assert!(!no.success, "a bare Enter declines");
    assert!(no.stdout.contains("anyway?"), "{}", no.stdout);
    assert!(
        no.stderr.contains("may not work as expected"),
        "{}",
        no.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );

    let yes = sb.run(&["meld", &spec], Some("y\n"), &[("MIND_TTY", "1")]);
    assert!(yes.success, "{}", yes.stderr);
    assert!(
        !sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
}

#[test]
fn meld_json_on_a_terminal_refuses_a_source_requiring_a_newer_mind_without_asking() {
    // spec: DSC-116 LIFE-45
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"9.0\"\n");
    let r = sb.run(
        &["meld", &sb.source_spec(), "--json"],
        Some("y\n"),
        &[("MIND_TTY", "1")],
    );
    assert!(!r.success, "declined, as with no terminal");
    assert!(!r.stdout.contains("anyway?"), "{}", r.stdout);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "the reply on stdin is never read"
    );
}

#[test]
fn meld_names_the_version_gap_when_a_newer_mind_toml_does_not_parse() {
    // spec: DSC-117
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\nmin-mind-version = \"9.0\"\nfuture-key = true\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(!r.success, "an unreadable mind.toml cannot be melded");
    assert!(r.stderr.contains("requires mind >= 9.0"), "{}", r.stderr);
    assert!(r.stderr.contains("future-key"), "{}", r.stderr);
    assert!(r.stderr.contains("mind evolve"), "{}", r.stderr);
}

#[test]
fn example_starter_convention_discovery() {
    // spec: DSC-10, DSC-11, DSC-12, DSC-20, CLI-85, WF-1
    // The starter example ships no mind.toml: items are found by convention and
    // their descriptions come from each item's frontmatter (a workflow's from
    // its `meta` object instead, WF-4).
    let sb = Sandbox::from_example("starter");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    // probe falls back to the listing on a non-TTY (piped) stdout; the
    // convention items appear with their kinds.
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("skill:greet"), "{}", probe.stdout);
    assert!(probe.stdout.contains("agent:scribe"), "{}", probe.stdout);
    assert!(probe.stdout.contains("rule:tone"), "{}", probe.stdout);
    assert!(probe.stdout.contains("workflow:hello"), "{}", probe.stdout);

    // A query that matches only a description (CLI-85): "plain" is in tone's
    // frontmatter description, not its name.
    let by_desc = sb.mind(&["probe", "plain"]);
    assert!(by_desc.success, "{}", by_desc.stderr);
    assert!(by_desc.stdout.contains("rule:tone"), "{}", by_desc.stdout);
    assert!(
        !by_desc.stdout.contains("agent:scribe"),
        "a description-only match should not list unrelated items: {}",
        by_desc.stdout
    );

    // Installing a convention item links it from the store.
    assert!(sb.mind(&["learn", "greet"]).success);
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "greet should be copied into the store"
    );

    // spec: WF-1, WF-10 -- a workflow discovered by convention (workflows/hello.js)
    // installs to the store and links into the agent home like any other kind.
    let learn_wf = sb.mind(&["learn", "hello"]);
    assert!(learn_wf.success, "{}\n{}", learn_wf.stdout, learn_wf.stderr);
    assert!(
        sb.mind_home.join("store/workflow/hello").exists(),
        "hello workflow should be copied into the store"
    );
    let wf_link = sb.claude_home.join("workflows/hello.js");
    assert!(
        std::fs::symlink_metadata(&wf_link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "hello workflow must be linked at workflows/hello.js: {:?}",
        wf_link
    );
}

#[test]
fn root_mindfile_exposes_hello() {
    // spec: DSC-1, DSC-50
    // The repo-root mind.toml sets roots = ["examples/hello"], so melding the
    // mind repo itself discovers the hello-mind skill by convention under that
    // root, and `mind learn hello-mind` links it into the agent home. Guards
    // the landing-page command `mind meld jaemk/mind`.
    // spec: DSC-35, DSC-54, DSC-58
    // The root mind.toml curates two skill libraries via [discover].sources
    // (substituted with local stand-ins by the fixture to stay offline). Melding
    // registers the whole chain register-only (install = false) while the repo's
    // own hello-mind item is still discovered by convention and installable.
    let sb = Sandbox::from_root_mindfile();
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:hello-mind"),
        "{}",
        probe.stdout
    );

    // DSC-54/DSC-58: the curated nested sources are registered (browsable) but
    // their items are NOT installed by default (install = false).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success, "{}", sources.stderr);
    assert!(
        sources.stdout.contains("anthropics-skills")
            && sources.stdout.contains("awesome-claude-skills"),
        "both curated sources must be registered: {}",
        sources.stdout
    );
    assert!(
        probe.stdout.contains("skill:astand"),
        "the curated stand-in's item must be available to browse: {}",
        probe.stdout
    );
    assert!(
        !sb.claude_home.join("skills/astand").exists(),
        "a register-only curated item must NOT be installed on meld"
    );

    let learn = sb.mind(&["learn", "hello-mind"]);
    assert!(learn.success, "{}", learn.stderr);
    assert!(
        sb.mind_home
            .join("store/skill/hello-mind/SKILL.md")
            .exists(),
        "hello-mind should be copied into the store"
    );
    assert!(
        sb.claude_home.join("skills/hello-mind").exists(),
        "hello-mind should be linked into the agent home"
    );
}

#[test]
fn example_monorepo_roots_discovery() {
    // spec: DSC-50, DSC-53
    // The monorepo example sets [source].roots, so convention discovery scans the
    // per-package subtrees and unions the results.
    let sb = Sandbox::from_example("monorepo");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "found under packages/web: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:release"),
        "found under packages/cli: {}",
        probe.stdout
    );
}

#[test]
fn example_explicit_inventory_offers_only_listed() {
    // spec: DSC-3
    // The explicit example declares a [[items]] inventory, which is authoritative:
    // convention is off and a shipped-but-unlisted file is not offered.
    let sb = Sandbox::from_example("explicit");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("rule:style"), "{}", probe.stdout);
    assert!(probe.stdout.contains("skill:scan"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("internal"),
        "an unlisted file is not offered: {}",
        probe.stdout
    );
}

#[test]
fn example_discover_kind_globs() {
    // spec: DSC-33, DSC-37
    // The discover example declares an authoritative [discover] with per-kind
    // include/exclude globs. A skill glob ends at SKILL.md (item = parent dir)
    // and an agent glob matches the .md (item = stem) (DSC-33); an exclude glob
    // drops a matched path (DSC-37). Convention scanning is off, so only the two
    // glob-matched items are offered.
    let sb = Sandbox::from_example("discover");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:alpha"),
        "skill glob matches packages/a/skills/alpha/SKILL.md, item = parent dir (DSC-33): {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta"),
        "agent glob matches packages/b/agents/beta.md, item = stem (DSC-33): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("secret"),
        "internal/skills/secret/SKILL.md is dropped by the exclude glob (DSC-37): {}",
        probe.stdout
    );
    // Exactly the two glob-matched items: convention scanning stays off.
    assert_eq!(
        probe.stdout.matches("skill:").count() + probe.stdout.matches("agent:").count(),
        2,
        "only the two glob-matched items are discovered: {}",
        probe.stdout
    );
}

#[test]
fn example_super_source_validates() {
    // spec: DSC-38, DSC-39
    // The super-source example declares a [discover].sources registry. It
    // validates clean structurally (review does not clone the nested chain).
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/super-source");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(
        r.success,
        "super-source example must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn source_directive_follow_branch_applies_when_no_consumer_flag() {
    // spec: DSC-41, CLI-17 (directive supplies default when no consumer flag)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-directive-follow");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with directive: {}", r.stderr);

    // Directive follow-branch=stable => clone at stable (sha_v1).
    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_v1,
        "directive follow-branch=stable should land on sha_v1"
    );

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("follow-branch"),
        "pin kind should be follow-branch: {pin_json}"
    );
}

#[test]
fn consumer_flag_overrides_source_directive() {
    // spec: DSC-41, CLI-17 (consumer flag overrides directive)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-override");
    // Directive says follow stable (sha_v1); consumer says --follow-branch main.
    // Adding the mind.toml advances main by one more commit, so we capture the
    // resulting tip AFTER that commit.
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    // sha_main_tip is HEAD of main after the mind.toml commit.
    let sha_main_tip = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let spec = sb.source_spec();

    // Consumer says --follow-branch main which overrides the directive.
    let r = sb.mind(&["meld", &spec, "--follow-branch", "main"]);
    assert!(r.success, "meld override: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_main_tip,
        "consumer --follow-branch main should override directive and land on main tip"
    );
    // Verify directive sha_v1 was NOT used (different commit).
    assert_ne!(
        commit, sha_v1,
        "directive must not take precedence over consumer flag"
    );
}

#[test]
fn source_directive_conflict_is_error() {
    // spec: DSC-41 (more than one pin directive is a MindToml error)
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"main\"\npin-tag = \"v1.0\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success, "conflicting directives should fail meld");
    assert!(
        r.stderr.contains("conflicting pin") || r.stderr.contains("mind.toml"),
        "expected pin conflict error: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered"
    );
}

#[test]
fn source_directive_pin_tag_applies_when_no_consumer_flag() {
    // spec: DSC-41 - a `pin-tag` directive supplies the default pin when the
    // consumer gives no flag (parity with the follow-branch directive test).
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-directive-tag");
    sb.write_and_commit("mind.toml", "[source]\npin-tag = \"v1.0\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with pin-tag directive: {}", r.stderr);

    // The directive lands the clone on the tagged commit (sha_v1), not main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "directive pin-tag=v1.0 should land on the tagged commit"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\""),
        "pin kind should be tag: {pin_json}"
    );
    assert!(
        pin_json.contains("v1.0"),
        "pin value should be v1.0: {pin_json}"
    );
}

#[test]
fn source_directive_pin_ref_applies_when_no_consumer_flag() {
    // spec: DSC-41 - a `pin-ref` directive supplies the default pin when the
    // consumer gives no flag.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-directive-ref");
    // The directive must name a commit that exists in the default-branch clone,
    // since the directive is read from the default-branch mind.toml. sha_v1 is an
    // ancestor of main tip, so it is reachable.
    sb.write_and_commit("mind.toml", &format!("[source]\npin-ref = \"{sha_v1}\"\n"));
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with pin-ref directive: {}", r.stderr);

    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "directive pin-ref should land on the named commit"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "pin kind should be ref: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v1),
        "pin value should be the sha: {pin_json}"
    );
}

#[test]
fn consumer_pin_ref_overrides_follow_branch_directive() {
    // spec: DSC-41, CLI-17 - a consumer flag of a DIFFERENT kind overrides the
    // directive (not just same-kind override). Directive follows `stable`; the
    // consumer pins a specific ref instead.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-cross-override");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();

    // Consumer pins the main-tip commit (after the mind.toml commit).
    let sha_main_tip = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let r = sb.mind(&["meld", &spec, "--pin-ref", &sha_main_tip]);
    assert!(r.success, "meld cross-kind override: {}", r.stderr);

    assert_eq!(
        read_source_commit(&sb),
        sha_main_tip,
        "consumer --pin-ref must override the follow-branch directive"
    );
    assert_ne!(
        read_source_commit(&sb),
        sha_v1,
        "the stable directive must not win over the consumer ref"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "persisted pin kind should be the consumer's ref, not follow-branch: {pin_json}"
    );
}

#[test]
fn meld_rejects_unknown_source_pin_field() {
    // spec: DSC-41 - `[source]` is `deny_unknown_fields`, so a misspelled pin
    // directive (e.g. `pin-branch` instead of `follow-branch`) is a parse error,
    // not a silently-ignored field that would leave the source on the default.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-unknown-field");
    sb.write_and_commit("mind.toml", "[source]\npin-branch = \"stable\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "an unknown [source] field must fail meld: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("mind.toml") || r.stderr.contains("pin-branch"),
        "expected a mind.toml parse error naming the bad field: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered after a mind.toml parse error"
    );
}

#[test]
fn meld_root_persists_in_sources_json_and_probe_shows_subtree_items() {
    // spec: DSC-51, STO-17, CLI-16
    // A sandbox whose items live under a subdirectory "sub/".
    let sb = Sandbox::bare("subtree");
    // Items under "sub/" only.
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy skill\n---\n# deploy\n",
    );
    sb.write_and_commit(
        "sub/agents/ops.md",
        "---\ndescription: ops agent\n---\n# ops\n",
    );
    // Nothing at the repo root (no conventional dirs).
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--root", "sub"]);
    assert!(r.success, "meld --root: {}", r.stderr);

    // The root is persisted in sources.json.
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("sub"),
        "roots should be persisted: {roots_json}"
    );

    // probe shows the subtree items.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "subtree skill: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:ops"),
        "subtree agent: {}",
        probe.stdout
    );
}

#[test]
fn meld_root_on_authoritative_source_prints_note() {
    // spec: DSC-52 - --root on an authoritative source prints a note and is ignored.
    let sb = Sandbox::bare("auth-source");
    sb.write_and_commit(
        "pkg/style.md",
        "---\ndescription: style rule\n---\n# style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"pkg/style.md\"\n",
        ),
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "pkg"]);
    assert!(
        r.success,
        "meld should succeed even with ignored --root: {}",
        r.stderr
    );
    // The note appears on stdout.
    assert!(
        r.stdout.contains("ignored") || r.stdout.contains("note"),
        "expected an 'ignored' note: {}",
        r.stdout
    );
    // The explicit item is still discovered via the authoritative mind.toml.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:style"),
        "authoritative item still discovered: {}",
        probe.stdout
    );
}

#[test]
fn meld_root_nonexistent_dir_exits_nonzero() {
    // spec: DSC-52 (last sentence), CLI-16 - a --root that is not a directory in
    // the clone is an InvalidRoot error and exits non-zero.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "does-not-exist"]);
    assert!(!r.success, "meld with missing root must fail");
    assert!(
        r.stderr.contains("InvalidRoot") || r.stderr.contains("not a directory"),
        "expected InvalidRoot error: {}",
        r.stderr
    );
    // Nothing is registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered after an invalid root"
    );
}

#[test]
fn two_root_flags_union_and_both_persist() {
    // spec: DSC-51, DSC-53, STO-17, CLI-16
    // `meld --root a --root b` is repeatable: both subtrees are scanned and
    // unioned, and BOTH roots are persisted in sources.json. Drives the real CLI
    // arg parsing (the unit tests set Source.roots directly, so this is the only
    // check that the repeated flag actually threads through).
    let sb = Sandbox::bare("two-roots");
    sb.write_and_commit(
        "a/skills/alpha/SKILL.md",
        "---\ndescription: alpha skill\n---\n# alpha\n",
    );
    sb.write_and_commit(
        "b/agents/beta.md",
        "---\ndescription: beta agent\n---\n# beta\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--root", "a", "--root", "b"]);
    assert!(r.success, "meld --root a --root b: {}", r.stderr);

    // Both roots persisted.
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("\"a\""),
        "root a persisted: {roots_json}"
    );
    assert!(
        roots_json.contains("\"b\""),
        "root b persisted: {roots_json}"
    );

    // Both subtrees discovered.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha"),
        "root a item: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta"),
        "root b item: {}",
        probe.stdout
    );
}

#[test]
fn meld_absolute_root_exits_nonzero() {
    // spec: DSC-52, CLI-16
    // An absolute --root is rejected via the real CLI (the unit test exercises
    // scan_source directly; this confirms the binary surfaces InvalidRoot and
    // registers nothing).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "/tmp"]);
    assert!(!r.success, "absolute root must fail");
    assert!(
        r.stderr.contains("InvalidRoot") || r.stderr.contains("not a directory"),
        "expected InvalidRoot: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing registered after an absolute root"
    );
}

#[test]
fn mindfile_roots_discovered_without_flag() {
    // spec: DSC-50 - [source].roots in mind.toml is respected without any --root flag.
    let sb = Sandbox::bare("toml-roots");
    sb.write_and_commit(
        "toolbox/skills/pack/SKILL.md",
        "---\ndescription: pack skill\n---\n# pack\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nroots = [\"toolbox\"]\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with roots in mind.toml: {}", r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:pack"),
        "item under toolbox/ should be found: {}",
        probe.stdout
    );
}

// ---- review verb tests (CLI-130, CLI-131, CLI-132, CLI-133) -------------------

#[test]
fn meld_pin_tag_uses_pinned_mindfile_for_authoritativeness_not_default_branch() {
    // spec: DSC-52, DSC-41, STO-18
    //
    // Regression (M2, stale mindfile after pinned re-clone): meld step 1 clones
    // the default branch and reads its mind.toml; step 3 re-clones at the
    // resolved pin. The `is_authoritative` gate (which decides whether a
    // consumer `--root` is honored or ignored, DSC-52) must read the PINNED
    // mind.toml, not the default branch's.
    //
    // Here the TAGGED commit (v1.0) is NON-authoritative ([source] metadata
    // only) and ships its items under `sub/`, so `--root sub` must be honored.
    // The DEFAULT branch tip is AUTHORITATIVE ([[items]] present); if meld read
    // that stale file it would ignore `--root` and print the DSC-52 note.
    let sb = Sandbox::bare("pinned-authoritativeness");

    // --- Tagged commit (v1.0): non-authoritative mind.toml + item under sub/. ---
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy skill\n---\n# deploy\n",
    );
    sb.write_and_commit(
        "mind.toml",
        // [source] only: no [[items]] and no [discover] -> NOT authoritative,
        // so convention scanning (under the chosen --root) stays on.
        "[source]\ndescription = \"non-authoritative at v1.0\"\n",
    );
    git(&sb.source, &["tag", "v1.0"]);

    // --- Default branch tip: authoritative mind.toml ([[items]] present). ---
    sb.write_and_commit(
        "pkg/style.md",
        "---\ndescription: style rule\n---\n# style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"authoritative at main tip\"\n\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"pkg/style.md\"\n",
        ),
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0", "--root", "sub"]);
    assert!(r.success, "meld --pin-tag v1.0 --root sub: {}", r.stderr);

    // The pinned (non-authoritative) file means --root is HONORED, so the
    // DSC-52 "ignored" note must NOT print (it would if the default branch's
    // authoritative file were read).
    assert!(
        !r.stdout.contains("ignored"),
        "--root must be honored against the pinned non-authoritative mind.toml, \
         not ignored against the default branch's authoritative one: {}",
        r.stdout
    );

    // And the root is actually persisted (only happens on the non-authoritative path).
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("sub"),
        "root from the pinned file must be persisted: {roots_json}"
    );

    // The pinned description (not the default branch's) is recorded.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("non-authoritative at v1.0"),
        "pinned [source].description should be recorded: {}",
        sources.stdout
    );
    assert!(
        !sources.stdout.contains("authoritative at main tip"),
        "default branch description must not leak through: {}",
        sources.stdout
    );

    // The pinned subtree item is discovered; the default branch's item is not.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "pinned subtree item should be discovered: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("rule:style"),
        "default branch's authoritative item must not appear: {}",
        probe.stdout
    );
}

#[test]
fn meld_pin_tag_uses_pinned_mindfile_for_nested_discovery_not_default_branch() {
    // spec: DSC-52, DSC-41, STO-18
    //
    // Companion to the authoritativeness regression: the nested
    // [discover].sources loop must also read the PINNED mind.toml, not the
    // default branch's. The default branch declares a nested source that does
    // not exist on disk; if meld read it, the recursive meld would fail. The
    // tagged commit declares no nested sources, so meld must succeed and pull in
    // exactly one source.
    let sb = Sandbox::bare("pinned-nested-discovery");

    // Tagged commit: a plain non-authoritative mind.toml, no nested sources,
    // one convention item.
    sb.write_and_commit("agents/dev.md", "---\ndescription: dev agent\n---\n# dev\n");
    sb.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"no nested sources at v1.0\"\n",
    );
    git(&sb.source, &["tag", "v1.0"]);

    // Default branch tip: declares a nested source pointing at a path that does
    // not exist, which would make a recursive meld fail if it were read.
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"nested at main tip\"\n\n",
            "[[discover.sources]]\n",
            "source = \"/nonexistent/nested/repo\"\n",
        ),
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(
        r.success,
        "meld must use the pinned (no-nested) mind.toml and succeed: {} {}",
        r.stdout, r.stderr
    );

    // Exactly one source was melded (no phantom nested source from the default
    // branch). recall --sources lists the single pinned source.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no nested sources at v1.0"),
        "pinned source description should be present: {}",
        sources.stdout
    );
    assert!(
        !sources.stdout.contains("/nonexistent/nested/repo"),
        "default branch's nested source must not be melded: {}",
        sources.stdout
    );
}

// --- managed policy enforcement (POL-*) ------------------------------------
//
// The policy is injected via $MIND_POLICY_FILE, which `Policy::load` honors only
// when no system policy file exists at the fixed per-OS path (POL-2). The test
// environment has no such system file, so the env var is authoritative here.
// Non-policy tests above never set MIND_POLICY_FILE, so they stay unmanaged
// (POL-4 inert). A local path-melded source's identity is `local/<base>/<name>`
// (see source.rs make_source / Sandbox::base_name), where <base> is the dynamic
// temp-dir name; the allow patterns below use `local/*/<name>` so the single
// segment wildcard matches that base deterministically without hardcoding it.

/// Build an un-onboarded nested source (no mind.toml) whose items live under a
/// `pkg/` subdir, so default convention discovery (repo root) finds nothing but a
/// curator-supplied `roots = ["pkg"]` does. It carries a `stable` branch (for a
/// curator follow-branch pin) holding the same content. Returns the sandbox.
fn make_unonboarded_nested(name: &str) -> Sandbox {
    let sb = Sandbox::bare(name);
    // An item under pkg/ only: the repo root holds no skills/agents/rules dir, so
    // a root-only scan discovers nothing.
    write(
        &sb.source.join("pkg/skills/widget/SKILL.md"),
        "---\nname: widget\ndescription: A curated widget skill\n---\n# widget\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "pkg layout"]);
    // A stable branch at this same content, so a follow-branch pin resolves.
    git(&sb.source, &["branch", "stable"]);
    sb
}

#[test]
fn curator_applies_follow_branch_roots_and_hook_when_nested_has_no_mind_toml() {
    // spec: DSC-59 DSC-60 DSC-61
    // A super-source curates an un-onboarded nested source (no mind.toml of its
    // own), supplying follow-branch, roots, and a hook. All three apply: the
    // nested source's pin is recorded as follow-branch, roots govern discovery
    // (the pkg-only item is found), and the hook runs.
    let nested = make_unonboarded_nested("widgets");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-61: roots applied -> the pkg-only item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "curator roots must govern discovery so the pkg-only item is found: {}",
        probe.stdout
    );

    // DSC-61: follow-branch applied -> the nested source's recorded pin is
    // follow-branch=stable. (The registry super-source itself has no pin
    // directive, so a follow-branch/stable pin can only be the nested source's.)
    let json = read_sources_json(&registry);
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "the nested source's pin must be recorded as follow-branch=stable: {json}"
    );

    // DSC-61: the hook ran in the nested source's clone (a follow-branch pin
    // snapshots a local source under the sources tree).
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("widgets");
    let marker = nested_clone.join("curated-hookran");
    assert!(
        marker.exists(),
        "the curator-supplied hook must have run in the nested clone: {} missing",
        marker.display()
    );
    // The hook command is recorded against the nested source.
    assert!(
        json.contains("touch curated-hookran"),
        "the curator hook command must be recorded on the nested source: {json}"
    );
}

#[test]
fn curator_values_ignored_with_warning_when_nested_has_mind_toml() {
    // spec: DSC-59 DSC-60 DSC-65
    // DSC-65: the curator's pin directive is AUTHORITATIVE and applies even when
    // the nested source ships its own mind.toml. DSC-60: the gated fields (roots
    // and hooks) are still suppressed when the source has a mind.toml, and the
    // warning fires only because those gated fields are present.
    let nested = make_unonboarded_nested("onboarded");
    // The nested source onboards with a mind.toml declaring its own roots (the
    // repo root), which closes the gate. It still ships the pkg-only item,
    // which a root scan won't find.
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nroots = [\".\"]\n",
    );
    // Point stable at the onboarded commit so the curator follow-branch can apply.
    git(&nested.source, &["branch", "-f", "stable"]);

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: a warning fires because roots/hooks are gated and suppressed.
    // The warning must mention the source name and "ignored"; it need NOT mention
    // the pin (DSC-65 exempts the pin from the warning).
    assert!(
        r.stderr.contains("declares its own discovery or hooks")
            && r.stderr.contains("ignored")
            && r.stderr.contains("onboarded"),
        "a DSC-60 warning must be emitted naming the onboarded source: {}",
        r.stderr
    );
    // This curator entry has no `add-roots`, so the actionable add-roots hint
    // must not be appended (it would be misleading advice for a suppression
    // caused by `roots`/hooks, not `add-roots`).
    assert!(
        !r.stderr.contains("--add-root"),
        "no add-roots hint should be printed when the curator entry set no \
         add-roots: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // DSC-65: the curator follow-branch IS now applied (authoritative). The nested
    // source's recorded pin must be follow-branch=stable, NOT the default branch.
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "the curator follow-branch must apply (authoritative DSC-65), recorded as follow-branch=stable: {json}"
    );
    // The suppressed roots: the pkg-only item is not discovered (a root scan of
    // the onboarded source finds nothing under the repo root). Roots are gated.
    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:widget"),
        "the curator roots must be suppressed: the pkg-only item must not appear: {}",
        probe.stdout
    );
    // The suppressed hook: it never ran and is not recorded. Hooks are gated.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("onboarded");
    assert!(
        !nested_clone.join("curated-hookran").exists(),
        "the curator hook must be suppressed (no marker)"
    );
    assert!(
        !json.contains("touch curated-hookran"),
        "the curator hook command must not be recorded when suppressed: {json}"
    );
}

#[test]
fn curator_add_roots_applied_when_nested_has_no_mind_toml() {
    // spec: DSC-88 -- a curator's `add-roots` on a `[discover].sources` entry
    // composes with an un-onboarded nested source's convention discovery,
    // exactly like `roots`/`flat-skills`/hooks (DSC-59/DSC-61), when the nested
    // source ships no mind.toml of its own.
    let nested = Sandbox::bare("addroot-widgets");
    nested.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: base skill\n---\n# base\n",
    );
    // Only reachable via the curator's add-roots entry.
    nested.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: add-root contributed skill\n---\n# addon\n",
    );
    let registry = Sandbox::bare("addroot-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\nadd-roots = [\"contrib\"]\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    let probe = registry.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:base"),
        "the base item (plain convention scan) must be discovered: {probe}"
    );
    assert!(
        probe.contains("skill:addon"),
        "curator add-roots must apply when the nested source has no mind.toml: {probe}"
    );
}

#[test]
fn curator_add_roots_applied_when_nested_mind_toml_has_only_metadata() {
    // spec: DSC-60 DSC-88 -- a nested mind.toml with only `[source]` metadata
    // declares no discovery or hooks of its own, so the gate stays open: the
    // curator's add-roots composes, with no "ignored" warning.
    let nested = Sandbox::bare("addroot-metadata");
    nested.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: base skill\n---\n# base\n",
    );
    nested.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: add-root contributed skill\n---\n# addon\n",
    );
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nprefix = \"md\"\n",
    );
    let registry = Sandbox::bare("addroot-registry-metadata");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\nadd-roots = [\"contrib\"]\n",
            nested.source_spec()
        ),
    );
    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("ignored"),
        "no DSC-60 warning: {}",
        r.stderr
    );
    let probe = registry.mind(&["probe"]).stdout;
    assert!(
        probe.contains("base") && probe.contains("addon"),
        "the curator add-roots must apply: {probe}"
    );
}

#[test]
fn curator_add_roots_ignored_with_warning_when_nested_has_mind_toml() {
    // spec: DSC-60 DSC-88 -- a curator-supplied `add-roots` on a nested source
    // that ships its own mind.toml must be GATED exactly like
    // roots/flat-skills/hooks, not composed unconditionally (contrast the
    // direct-consumer `meld --add-root`, DSC-84, which is never gated). Without
    // this gate a curator could reach an item its author deliberately did not
    // export via an authoritative mind.toml (DSC-3): this is the C16 fix. A
    // mind.toml declaring its own `roots` closes the gate; convention scanning
    // of that root still finds `base`.
    let nested = Sandbox::bare("addroot-onboarded");
    nested.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: base skill\n---\n# base\n",
    );
    nested.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: add-root contributed skill\n---\n# addon\n",
    );
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nroots = [\".\"]\n",
    );

    let registry = Sandbox::bare("addroot-registry-gated");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\nadd-roots = [\"contrib\"]\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: a warning fires because add-roots is a gated field and is present.
    assert!(
        r.stderr.contains("declares its own discovery or hooks")
            && r.stderr.contains("ignored")
            && r.stderr.contains("onboarded"),
        "a DSC-60 warning must be emitted naming the onboarded source: {}",
        r.stderr
    );
    // The suppression of a curator's `add-roots` can silently install fewer
    // items than before; the warning must be actionable, not just a notice:
    // it names `add-roots` explicitly and points at the un-gated consumer
    // escape (`meld --add-root`, DSC-84) that reaches the same items.
    assert!(
        r.stderr.contains("add-roots")
            && r.stderr.contains("mind meld")
            && r.stderr.contains("--add-root")
            && r.stderr.contains("not gated"),
        "the DSC-60 warning must tell the consumer they can apply add-roots \
         themselves with `mind meld <source> --add-root <dir>`: {}",
        r.stderr
    );

    let probe = registry.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:base"),
        "the base item (plain convention scan) must still be discovered: {probe}"
    );
    assert!(
        !probe.contains("skill:addon"),
        "curator add-roots must be gated (ignored) when the nested source ships \
         its own mind.toml: {probe}"
    );
}

#[test]
fn consumer_pin_flag_overrides_curator_follow_branch() {
    // spec: DSC-61
    // DSC-41 precedence: a consumer `meld` pin flag still wins over a curator
    // follow-branch. The effective pin is `consumer_pin.or(curator_follow_pin)`,
    // so a consumer flag on a direct meld of the (otherwise curator-adopted)
    // un-onboarded source must record the consumer pin, not the curator branch.
    //
    // The un-onboarded source carries a `stable` branch (what a curator would
    // pin via follow-branch) and a `v1` tag (the consumer's explicit choice).
    // Melding it directly with --pin-tag v1 must record the tag pin: the consumer
    // flag wins. (A nested meld passes no consumer pin, so the curator branch is
    // what applies there; the apply test covers that positive path.)
    let nested = make_unonboarded_nested("pinned");
    git(&nested.source, &["tag", "v1"]);
    let spec = nested.source_spec();

    let r = nested.mind(&["meld", &spec, "--pin-tag", "v1"]);
    assert!(r.success, "meld --pin-tag should succeed: {}", r.stderr);
    let json = read_sources_json(&nested);
    assert!(
        json.contains("\"kind\": \"tag\"") && json.contains("v1"),
        "a consumer pin flag must win and record a tag pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "a consumer pin flag must override any follow-branch (no follow-branch pin recorded): {json}"
    );
}

#[test]
fn curator_empty_roots_list_discovers_nothing() {
    // spec: DSC-59 DSC-61
    // A curator `roots = []` (explicit empty list) is distinct from unset roots:
    // it scans zero roots, mirroring the source-level DSC-50/DSC-53 semantics.
    // The un-onboarded nested source's only item lives under pkg/, so with an
    // empty roots list nothing is discovered -- and crucially this differs from
    // omitting roots entirely (which would fall back to the repo root and still
    // find nothing here, so to make the empty-list behavior load-bearing we put
    // an item at the REPO ROOT too: an unset/repo-root scan would find it, while
    // an explicit empty list must scan nothing and find neither.)
    let nested = make_unonboarded_nested("emptyroots");
    // Add a root-level item. A repo-root scan (unset roots) would find this; an
    // explicit empty roots list must not.
    nested.write_and_commit(
        "skills/toplevel/SKILL.md",
        "---\nname: toplevel\ndescription: A root-level skill\n---\n# toplevel\n",
    );

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = []\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // An explicit empty roots list scans nothing: neither the pkg item nor the
    // root-level item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:widget"),
        "empty curator roots must scan nothing (pkg item must not appear): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:toplevel"),
        "empty curator roots must scan nothing, not even the repo root (toplevel must not appear): {}",
        probe.stdout
    );
}

#[test]
fn curator_hooks_do_not_leak_across_nested_entries() {
    // spec: DSC-59 DSC-61
    // Two un-onboarded nested sources, each with its own
    // `[[discover.sources.hooks]]`. Each entry's CuratedConfig is independent, so
    // a given nested source must run ONLY its own hook -- never the sibling
    // entry's. A leak would run both hooks in one clone.
    let first = make_unonboarded_nested("alpha");
    let second = make_unonboarded_nested("beta");

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch alpha-marker\"\n\n\
             [[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch beta-marker\"\n",
            first.source_spec(),
            second.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    let alpha_clone = registry
        .mind_home
        .join("sources/local")
        .join(first.base_name())
        .join("alpha");
    let beta_clone = registry
        .mind_home
        .join("sources/local")
        .join(second.base_name())
        .join("beta");

    // Each entry ran exactly its own hook in its own clone.
    assert!(
        alpha_clone.join("alpha-marker").exists(),
        "alpha's own hook must run in alpha's clone"
    );
    assert!(
        beta_clone.join("beta-marker").exists(),
        "beta's own hook must run in beta's clone"
    );
    // No leak: a sibling entry's hook must not have run in the other's clone.
    assert!(
        !alpha_clone.join("beta-marker").exists(),
        "beta's hook leaked into alpha's clone"
    );
    assert!(
        !beta_clone.join("alpha-marker").exists(),
        "alpha's hook leaked into beta's clone"
    );

    // And the recorded hook on each nested source is only its own command.
    let json = read_sources_json(&registry);
    assert!(
        json.contains("touch alpha-marker") && json.contains("touch beta-marker"),
        "each nested source records its own hook command: {json}"
    );
}

#[test]
fn curator_values_suppressed_when_nested_declares_own_pin_roots_hooks() {
    // spec: DSC-59 DSC-60 DSC-65
    // DSC-60 gate with a nested mind.toml that DECLARES its OWN pin/roots/hooks.
    // Under DSC-65: the CURATOR pin now wins over the source's own pin. Roots and
    // hooks remain gated: the source's own roots/hooks still govern (the curator's
    // roots/hooks are suppressed). The warning fires because gated values are present.
    let nested = make_unonboarded_nested("selfdeclared");
    // The nested source onboards declaring its OWN pin (follow-branch = own), its
    // own roots (pkg, where its item lives), and its own hook.
    nested.write_and_commit(
        "mind.toml",
        "[source]\n\
         description = \"self-declared\"\n\
         follow-branch = \"own\"\n\
         roots = [\"pkg\"]\n\n\
         [[hooks]]\n\
         run = \"touch source-own-hook\"\n",
    );
    git(&nested.source, &["branch", "own"]);
    git(&nested.source, &["branch", "-f", "stable"]);

    let registry = Sandbox::bare("registry");
    // The curator supplies a DIFFERENT follow-branch (stable), bogus roots, and a
    // curator hook. Under DSC-65: curator pin (stable) overrides source pin (own).
    // Roots and hooks are still gated: source's own roots/hooks govern.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"nonexistent\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curator-hook\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: the warning fires because roots/hooks are present and gated.
    assert!(
        r.stderr.contains("declares its own discovery or hooks")
            && r.stderr.contains("ignored")
            && r.stderr.contains("selfdeclared"),
        "a DSC-60 warning must name the onboarded source: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The source's OWN roots win (roots are gated): its pkg item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "the source's own roots = [pkg] must govern, finding its item: {}",
        probe.stdout
    );
    // DSC-65: the CURATOR follow-branch=stable now wins over the source's own
    // follow-branch=own. The recorded pin must be stable, NOT own.
    assert!(
        json.contains("follow-branch") && json.contains("\"stable\""),
        "the curator follow-branch=stable must win (DSC-65 authoritative): {json}"
    );
    assert!(
        !json.contains("\"own\""),
        "the source's own follow-branch=own must be overridden by the curator pin: {json}"
    );
    // The source's own hook ran; the curator's did not. Hooks are gated.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("selfdeclared");
    assert!(
        nested_clone.join("source-own-hook").exists(),
        "the source's own declared hook must run"
    );
    assert!(
        !nested_clone.join("curator-hook").exists(),
        "the curator hook must be suppressed"
    );
    assert!(
        json.contains("touch source-own-hook") && !json.contains("touch curator-hook"),
        "only the source's own hook command is recorded: {json}"
    );
}

#[test]
fn curator_pin_ref_authoritative_overrides_source_own_pin() {
    // spec: DSC-59 DSC-65
    // A curator entry with `pin-ref = <sha>` on a nested source that has its own
    // mind.toml (with its own `follow-branch = "own"`) must record the curator's
    // pin-ref as the nested source's effective pin (DSC-65: curator pin is
    // authoritative, regardless of the source's own mind.toml). Roots and hooks
    // from the curator are absent here so no DSC-60 warning fires.
    let nested = make_unonboarded_nested("pinref-target");
    // The nested source onboards with its own follow-branch pin pointing to "own".
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nfollow-branch = \"own\"\n",
    );
    git(&nested.source, &["branch", "own"]);

    // Capture the nested source's HEAD commit sha (the onboarded content commit).
    let sha_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&nested.source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("git rev-parse HEAD");
    let sha = String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string();
    assert!(!sha.is_empty(), "could not capture HEAD commit sha");

    // The curator supplies pin-ref pointing at the onboarded commit.
    let registry = Sandbox::bare("pinref-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             pin-ref = \"{sha}\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-65: the curator pin-ref must apply even though the source has its own
    // mind.toml with its own follow-branch=own. No warning fires: no gated
    // fields (roots/hooks) are present.
    assert!(
        !r.stderr.contains("ignored"),
        "no DSC-60 warning should fire for a pin-only curator entry (no gated fields): {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The recorded pin must be the ref variant with the curator-supplied sha.
    assert!(
        json.contains("\"kind\": \"ref\"") && json.contains(&sha),
        "the curator pin-ref must be recorded as the ref pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "the source's own follow-branch must be overridden by the curator pin-ref: {json}"
    );
}

#[test]
fn curator_hook_skipped_under_non_tty_without_skip_flag() {
    // spec: DSC-61
    // The curator hook runs through the same disclosure/safety path as a source's
    // own hooks, INCLUDING the non-TTY skip (HOOK-22). The integration harness is
    // non-TTY (piped stdin), so a meld WITHOUT
    // `--accept-hooks=install,update,uninstall` must SKIP the curator hook (its
    // marker is never created) rather than run it silently, while the meld itself
    // still succeeds and the source is registered.
    let nested = make_unonboarded_nested("skiphook");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // No --accept-hooks=install,update,uninstall: non-TTY must take the skip path.
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld should still succeed: {} {}",
        r.stdout, r.stderr
    );

    // The hook was skipped, not run: no marker.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("skiphook");
    assert!(
        !nested_clone.join("curated-hookran").exists(),
        "a non-TTY meld without the skip flag must NOT run the curator hook"
    );
    // The skip is announced (HOOK-22 disclosure path), not silent.
    assert!(
        r.stdout.contains("skipped install hook") || r.stderr.contains("skipped install hook"),
        "the skip must be announced: {} {}",
        r.stdout,
        r.stderr
    );
    // The source is still registered (skip != abort), and the follow-branch pin
    // still applies (the gate is about hook execution, not pin/roots).
    let json = read_sources_json(&registry);
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "roots/follow-branch still apply even when the hook is skipped: {json}"
    );
}

#[test]
fn sync_rewalk_applies_curator_follow_branch_to_new_nested() {
    // spec: DSC-59 DSC-61
    // The DSC-57 sync re-walk threads CuratedConfig: a nested source newly added
    // to a super-source's [discover].sources, carrying a curator follow-branch,
    // is melded by `sync` with the same gate/apply behavior as a fresh meld. Its
    // recorded pin must be follow-branch=stable.
    let registry = Sandbox::bare("registry");
    let first = make_unonboarded_nested("present"); // listed from the start
    let later = make_unonboarded_nested("arriving"); // added before sync

    // Initially the super-source curates only `first`.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = [\"pkg\"]\n",
            first.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "initial meld should succeed: {} {}",
        r.stdout, r.stderr
    );

    // `arriving` is not yet registered.
    let before = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !before.contains("/arriving"),
        "the new nested source must not be registered before sync: {before}"
    );

    // Add `arriving` with a curator follow-branch to the super-source's list.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n",
            first.source_spec(),
            later.source_spec()
        ),
    );

    // sync re-walks and melds `arriving`, applying the curator follow-branch.
    let r = registry.mind(&["sync"]);
    assert!(r.success, "sync should succeed: {} {}", r.stdout, r.stderr);
    assert!(
        registry
            .mind(&["recall", "--sources"])
            .stdout
            .contains("/arriving"),
        "sync must register the newly-listed nested source"
    );

    // DSC-61 end-to-end through sync: the curator follow-branch is recorded as the
    // newly discovered source's pin. The registry super-source declares no pin and
    // `first` has none, so a follow-branch=stable pin can only be `arriving`'s.
    let json = read_sources_json(&registry);
    assert!(
        json.contains("arriving") && json.contains("follow-branch") && json.contains("stable"),
        "sync's re-walk must apply the curator follow-branch to the new nested source: {json}"
    );
    // Curator roots also applied through sync: the pkg-only item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "curator roots must govern discovery for a sync-discovered nested source: {}",
        probe.stdout
    );
}

#[test]
fn curator_pin_tag_authoritative_overrides_source_own_pin() {
    // spec: DSC-59 DSC-65
    // A curator entry with `pin-tag = <tag>` on a nested source that ships its
    // own mind.toml (with its own `follow-branch = "own"`) must record the
    // curator's tag pin as the nested source's effective pin (DSC-65: curator pin
    // is authoritative regardless of the source's own mind.toml). This covers the
    // pin-tag kind, complementing the pin-ref and follow-branch cases. No gated
    // fields (roots/hooks) are present, so no DSC-60 warning fires.
    let nested = make_unonboarded_nested("pintag-target");
    // The nested source onboards with its own follow-branch pin pointing to "own".
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nfollow-branch = \"own\"\n",
    );
    git(&nested.source, &["branch", "own"]);
    // A tag at the onboarded commit, which the curator pins to.
    git(&nested.source, &["tag", "rel-1"]);

    let registry = Sandbox::bare("pintag-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             pin-tag = \"rel-1\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // No DSC-60 warning: a pin-only curator entry has no gated fields.
    assert!(
        !r.stderr.contains("ignored"),
        "no DSC-60 warning should fire for a pin-only (pin-tag) curator entry: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The recorded pin must be the tag variant with the curator-supplied tag.
    assert!(
        json.contains("\"kind\": \"tag\"") && json.contains("rel-1"),
        "the curator pin-tag must be recorded as the tag pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "the source's own follow-branch must be overridden by the curator pin-tag: {json}"
    );
}

#[test]
fn curator_conflicting_pin_directives_is_meld_error() {
    // spec: DSC-59
    // A `[discover.sources]` entry that declares more than one pin directive
    // (here follow-branch AND pin-ref) is a MindToml one-of error. The error must
    // surface at meld (not only as a unit test), and nothing must be registered.
    let nested = make_unonboarded_nested("conflict-target");
    let registry = Sandbox::bare("conflict-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             pin-ref = \"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "a nested entry with two pin directives must fail at meld: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("conflicting pin"),
        "the meld error must mention conflicting pin directives: {}",
        r.stderr
    );
    // The conflicting nested source must NOT be registered.
    let recall = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !recall.contains("/conflict-target"),
        "the conflicting nested source must not be registered: {recall}"
    );
}

// ---- DEP-4, DEP-5, DEP-6: `requires:` frontmatter dependency key ----------

/// A consumer `--pin-tag` value that starts with `-` (e.g. `--pin-tag=-x`) must
/// be rejected with `InvalidRef` before any git call. No source must be registered.
/// The `=` form (--pin-tag=-x) passes the value through clap as a string; our
/// `resolve_pin_flags` then validates it before any git subprocess is spawned.
#[test]
fn meld_pin_tag_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-tag");
    let spec = sb.source_spec();

    // Use the `--flag=value` form so clap passes the leading-dash value through
    // to our validate_ref_value call rather than treating it as a flag itself.
    let r = sb.mind(&["meld", &spec, "--pin-tag=-x"]);
    assert!(
        !r.success,
        "--pin-tag=-x must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The error must mention the invalid ref, not a git error.
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    // No source registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after invalid pin: {}",
        sources.stdout
    );
}

/// A consumer `--pin-ref` value that starts with `-` must be rejected.
#[test]
fn meld_pin_ref_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-ref");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-ref=--upload-pack=evil"]);
    assert!(
        !r.success,
        "--pin-ref=--upload-pack=evil must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered: {}",
        sources.stdout
    );
}

/// A consumer `--follow-branch` value that starts with `-` must be rejected.
#[test]
fn meld_follow_branch_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-branch");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--follow-branch=-evil"]);
    assert!(
        !r.success,
        "--follow-branch=-evil must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered: {}",
        sources.stdout
    );
}

/// The space-separated form `--pin-tag -x` (leading-dash value) is rejected at
/// the clap layer (no `allow_hyphen_values`), so clap treats `-x` as a flag and
/// errors before `resolve_pin_flags`. This is the complement of the `=` form
/// (`--pin-tag=-x`), which clap passes through to `validate_ref_value`. Either
/// surface must end with no source registered and no git fetch having run: a
/// leading-dash injection cannot reach a git subprocess by either path.
#[test]
fn meld_pin_tag_space_separated_leading_dash_is_rejected_before_git() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-space");
    let spec = sb.source_spec();

    // Space-separated leading-dash value: clap rejects this as an unknown flag /
    // missing value rather than accepting `-x` as the tag's value.
    let r = sb.mind(&["meld", &spec, "--pin-tag", "-x"]);
    assert!(
        !r.success,
        "--pin-tag -x (space form) must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No source registered and no fetch ran (the failure is before any git call,
    // whether at the clap layer or via validate_ref_value).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after a rejected space-form pin: {}",
        sources.stdout
    );
}

// ---- json-mode meld (CLI-156) -----------------------------------------------

#[test]
fn meld_flat_skills_flag_discovers_root_level_skill_dirs() {
    // spec: DSC-74 DSC-75 CLI-158 STO-44
    // `meld --flat-skills` discovers skills as bare-name directories at the repo
    // root (no `skills/` container), records the override on the source, and
    // leaves agent discovery (a conventional `agents/` dir) unchanged.
    let sb = make_flat_source("flatsrc", None);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--flat-skills", "--yes"]);
    assert!(
        r.success,
        "meld --flat-skills failed: {} {}",
        r.stdout, r.stderr
    );

    let recall = sb.mind(&["recall"]);
    for item in ["alpha", "beta", "dev"] {
        assert!(
            recall.stdout.contains(item),
            "flat skill/agent '{item}' must be discovered: {}",
            recall.stdout
        );
    }

    // STO-44: the consumer override is persisted on the source.
    let json = read_sources_json(&sb);
    assert!(
        json.contains("\"flat_skills\": true"),
        "the --flat-skills override must be persisted on the source: {json}"
    );
}

#[test]
fn meld_without_flat_skills_skips_root_level_skill_dirs() {
    // spec: DSC-74
    // Control: without the flag (and with no `[source].flat-skills`), the same
    // root-level skill directories are NOT discovered (the DSC-10 container layout
    // is required), while the conventional `agents/` dir still yields its item.
    let sb = make_flat_source("flatsrc-control", None);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "root-level skill dirs must NOT be discovered without flat-skills: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:dev"),
        "the conventional agents/ item must still be discovered: {}",
        probe.stdout
    );
}

#[test]
fn source_flat_skills_directive_discovers_without_flag() {
    // spec: DSC-74
    // A source that declares `[source].flat-skills = true` (non-authoritative
    // mind.toml) gets flat skill discovery with no consumer flag.
    let sb = make_flat_source("flatsrc-declared", Some("[source]\nflat-skills = true\n"));
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && probe.stdout.contains("skill:beta"),
        "[source].flat-skills must enable flat discovery without a flag: {}",
        probe.stdout
    );
}

#[test]
fn meld_flat_skills_ignored_for_authoritative_mindfile() {
    // spec: DSC-76
    // For an authoritative mind.toml (declaring [[items]]), --flat-skills affects
    // nothing and `meld` prints a note that it is ignored. The explicitly declared
    // item is found; the root-level flat dirs are not.
    let toml = "[[items]]\nkind = \"skill\"\nname = \"alpha\"\npath = \"alpha\"\n";
    let sb = make_flat_source("flatsrc-auth", Some(toml));
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--flat-skills", "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("--flat-skills is ignored"),
        "an authoritative mind.toml must note that --flat-skills is ignored: {}",
        r.stdout
    );

    // Authoritative discovery: only the declared `alpha`, and `beta` (a sibling
    // root dir) is NOT scanned in.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "authoritative mind.toml must ignore the flat layout: {}",
        probe.stdout
    );
}

#[test]
fn curator_flat_skills_applies_when_nested_has_no_mind_toml() {
    // spec: DSC-77
    // A super-source curates an un-onboarded nested flat-layout source (no
    // mind.toml of its own), supplying `flat-skills = true`. The flag applies
    // (the DSC-60 gate permits it), so the nested source's root-level skill dirs
    // are discovered.
    let nested = make_flat_source("flat-nested", None);
    let registry = Sandbox::bare("registry-flat");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             flat-skills = true\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && probe.stdout.contains("skill:beta"),
        "curator flat-skills must govern discovery of the un-onboarded nested source: {}",
        probe.stdout
    );
}

#[test]
fn curator_flat_skills_ignored_with_warning_when_nested_has_mind_toml() {
    // spec: DSC-77 DSC-60
    // When the nested source ships its own mind.toml, a curator-supplied
    // `flat-skills = true` is gated out (the DSC-60 gate, closed by the
    // mind.toml's own `roots`) and a warning fires. The nested mind.toml does
    // not declare flat-skills, so its root-level skill dirs are NOT discovered;
    // its conventional agents/ item still is.
    let nested = make_flat_source(
        "flat-onboarded",
        Some("[source]\ndescription = \"onboarded\"\nroots = [\".\"]\n"),
    );
    let registry = Sandbox::bare("registry-flat-gated");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             flat-skills = true\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: the warning fires (flat-skills is a gated field) and names the source.
    assert!(
        r.stderr.contains("declares its own discovery or hooks")
            && r.stderr.contains("ignored")
            && r.stderr.contains("flat-onboarded"),
        "a DSC-60 warning must be emitted naming the onboarded source: {}",
        r.stderr
    );

    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "curator flat-skills must be suppressed: root-level skill dirs must not appear: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:dev"),
        "the nested source's conventional agents/ item must still be discovered: {}",
        probe.stdout
    );
}

#[test]
fn sync_rewalk_respects_namespace_key_in_mindfile() {
    // spec: DSC-78
    // A super-source whose [discover].sources entry uses `namespace = "pfx"`
    // (the canonical DSC-78 key, distinct from the legacy `as = "pfx"`) must
    // register the nested source under that namespace alias during sync re-walk.
    // Guards H2: the re-walk previously passed `ns.alias` (the legacy `as`
    // field), silently dropping any value set via the canonical `namespace` key.
    let nested = Sandbox::named("dsc78-n");
    let super_src = Sandbox::bare("dsc78-sup");

    // Start with no discover sources so the nested source is NOT registered
    // at initial meld time.
    super_src.write_and_commit("mind.toml", "[source]\ndescription = \"super\"\n");

    let r = super_src.mind(&["meld", &super_src.source_spec()]);
    assert!(
        r.success,
        "meld super-source failed: {} {}",
        r.stdout, r.stderr
    );

    // Confirm nested is not yet registered.
    let before = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        !before.contains("/dsc78-n"),
        "nested must not be registered before sync re-walk: {before}"
    );

    // Update super-source mind.toml to add the nested source using the
    // canonical `namespace` key (not `as`).
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[source]\ndescription = \"super\"\n\
             [[discover.sources]]\nsource = \"{}\"\nnamespace = \"pfx\"\n",
            nested.source_spec()
        ),
    );

    // sync fetches the updated super-source mind.toml and re-walks discover.sources,
    // finding the newly added nested source with namespace = "pfx".
    let r = super_src.mind(&["sync"]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);

    let after = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        after.contains("/dsc78-n"),
        "nested source must be registered after sync re-walk: {after}"
    );
    assert!(
        after.contains("namespace:pfx"),
        "nested source must carry namespace:pfx alias after sync re-walk (DSC-78): {after}"
    );
}

// ---- CLI surface: flag renames and aliases (CLI-165, CLI-171 etc.) ----------

/// A source with a marketplace.json listing one in-repo plugin, plus skills
/// under `community/` the manifest does not list (one flat, one containered).
fn marketplace_with_unlisted_community() -> Sandbox {
    let sb = Sandbox::bare("mkt-addroot");
    sb.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"Cat","plugins":[{"name":"kit","source":"./plugins/kit"}]}"#,
    );
    sb.write_and_commit(
        "plugins/kit/skills/foo/SKILL.md",
        "---\ndescription: listed skill\n---\n# foo\n",
    );
    sb.write_and_commit(
        "community/extra/SKILL.md",
        "---\ndescription: unlisted flat skill\n---\n# extra\n",
    );
    sb.write_and_commit(
        "community/skills/bonus/SKILL.md",
        "---\ndescription: unlisted containered skill\n---\n# bonus\n",
    );
    sb
}

#[test]
fn add_root_composes_with_marketplace_manifest() {
    // spec: DSC-84 MKT-17 STO-55 CLI-197
    // --add-root does not suppress the manifest (contrast --root, MKT-15): the
    // manifest's items stay offered and the added root's unlisted skills (both
    // layouts) are offered alongside them. The roots persist on the source
    // (STO-55): probe is a separate process, so seeing the items proves the
    // scan re-reads them from sources.json.
    let sb = marketplace_with_unlisted_community();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", "community", "--register-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    // MKT-17: the MKT-15 suppression note must NOT fire for --add-root.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("manifest's plugin components are ignored"),
        "--add-root must not suppress the manifest: {combined}"
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:kit:foo"),
        "manifest item must stay offered under its plugin namespace: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:extra"),
        "unlisted flat skill under the added root must be offered: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:bonus"),
        "unlisted containered skill under the added root must be offered: {}",
        probe.stdout
    );

    // The composed items install through the normal learn path.
    let r = sb.mind(&["learn", "extra"]);
    assert!(r.success, "learn extra failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/extra").exists(),
        "add-root skill must link into the lobe"
    );
}

#[test]
fn add_root_on_single_plugin_source_inherits_plugin_prefix() {
    // spec: DSC-86
    // A single-plugin source with no explicit override namespaces its own
    // items under the plugin-name default (MKT-5); an added root's unlisted
    // items share that namespace instead of installing unprefixed.
    let sb = Sandbox::bare("plugin-addroot");
    sb.write_and_commit(".claude-plugin/plugin.json", r#"{"name":"kit"}"#);
    sb.write_and_commit(
        "skills/foo/SKILL.md",
        "---\ndescription: plugin skill\n---\n# foo\n",
    );
    sb.write_and_commit(
        "community/extra/SKILL.md",
        "---\ndescription: unlisted flat skill\n---\n# extra\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", "community", "--register-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:kit:foo"),
        "plugin item must carry the plugin-name prefix: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:kit:extra"),
        "add-root item must share the plugin-name prefix: {}",
        probe.stdout
    );
}

#[test]
fn add_root_items_carry_source_prefix_not_plugin_prefix() {
    // spec: DSC-86
    // On a MARKETPLACE source (per-entry namespaces, no single plugin), an
    // explicit --namespace layers over the manifest's per-plugin prefix
    // (MKT-13) while an add-root item gets only the source prefix.
    let sb = marketplace_with_unlisted_community();
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--namespace",
        "jk",
        "--add-root",
        "community",
        "--register-only",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:kit:foo"),
        "manifest item must carry outer + per-plugin prefix: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:jk:extra"),
        "add-root item must carry only the source prefix: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:jk:kit:extra"),
        "add-root item must not inherit a per-plugin prefix: {}",
        probe.stdout
    );
}

#[test]
fn add_root_composes_with_authoritative_mind_toml() {
    // spec: DSC-84
    // An authoritative mind.toml ignores --root (DSC-52) but composes with
    // --add-root: the declared item and the added root's skill are both
    // offered, with no ignored note.
    let sb = Sandbox::bare("auth-addroot");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"declared\"\npath = \"lib/declared\"\n",
    );
    sb.write_and_commit(
        "lib/declared/SKILL.md",
        "---\ndescription: declared skill\n---\n# declared\n",
    );
    sb.write_and_commit(
        "extra/skills/undeclared/SKILL.md",
        "---\ndescription: undeclared skill\n---\n# undeclared\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", "extra", "--register-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("--add-root is ignored") && !combined.contains("is ignored"),
        "--add-root must not be ignored for an authoritative mind.toml: {combined}"
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:declared"),
        "declared item must stay offered: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("skill:undeclared"),
        "add-root skill must compose with the authoritative inventory: {}",
        probe.stdout
    );
}

#[test]
fn add_root_extends_plain_convention_scan() {
    // spec: DSC-84
    // On a plain convention source, --add-root extends the effective root set:
    // the repo root's items and the added root's items are unioned.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "community/skills/side/SKILL.md",
        "---\ndescription: side skill\n---\n# side\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", "community", "--register-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:review") && probe.stdout.contains("skill:side"),
        "repo-root and add-root items must union: {}",
        probe.stdout
    );
}

#[test]
fn add_root_dedups_paths_the_manifest_already_contributed() {
    // spec: DSC-85
    // An added root that overlaps the manifest's in-repo plugin discovers the
    // same on-disk skill; the manifest entry wins (it keeps the per-plugin
    // namespace) and no duplicate or bare-name copy is offered.
    let sb = marketplace_with_unlisted_community();
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--add-root",
        "plugins/kit",
        "--register-only",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:kit:foo"),
        "the manifest entry must win the overlap: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:foo\n") && probe.stdout.matches("foo").count() == 1,
        "the overlapping skill must not be offered twice / bare: {}",
        probe.stdout
    );
}

#[test]
fn add_root_distinct_path_collision_is_duplicate_item() {
    // spec: DSC-85
    // A different on-disk path colliding on (kind, effective name) with a
    // base-layer item is a DuplicateItem error at meld.
    let sb = Sandbox::new(); // convention fixture ships skills/review
    sb.write_and_commit(
        "extra/skills/review/SKILL.md",
        "---\ndescription: colliding skill\n---\n# review\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", "extra", "--register-only"]);
    assert!(
        !r.success,
        "colliding add-root item must fail the meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("review"),
        "the DuplicateItem error must name the item: {}",
        r.stderr
    );
}

#[test]
fn add_root_invalid_root_is_error() {
    // spec: DSC-84 -- the InvalidRoot rules match --root.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--add-root",
        "no-such-dir",
        "--register-only",
    ]);
    assert!(!r.success, "a missing add-root dir must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("scan root") && r.stderr.contains("no-such-dir"),
        "the InvalidRoot error must name the root: {}",
        r.stderr
    );
}

// ---- CLI-231: `sync [source]` selector ------------------------------------

#[test]
fn an_item_with_an_unreadable_manifest_is_not_pruned() {
    // spec: DSC-123 -- the scan drops an item whose own mind.toml will not
    // parse (DSC-98), but the source still ships it: neither an upgrade of a
    // pruning source nor `mind prune` removes it, and `recall` does not call
    // it removed upstream.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    write(&sb.source.join("mind.toml"), "[source]\nprune = true\n");
    sb.write_and_commit("skills/review/mind.toml", "bogus = 1\n");

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("skipping skill 'review'"), "{}", r.stderr);
    assert!(!r.stdout.contains("pruned"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);

    let r = sb.mind(&["prune", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
    assert!(sb.claude_home.join("skills/review").exists());

    let r = sb.mind(&["recall"]);
    assert!(
        r.stdout.contains("(its manifest upstream is unreadable)"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("removed upstream"), "{}", r.stdout);

    let j = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let rows = j["items"][0]["items"].as_array().expect("items array");
    let review = rows
        .iter()
        .find(|r| r["key"] == "skill:review")
        .expect("the skipped skill is listed");
    assert_eq!(review["orphaned"], true, "{review}");
    assert_eq!(review["unreadable"], true, "{review}");

    let v = parse_json(&sb.mind(&["introspect", "--json"]).stdout);
    let kinds: Vec<&str> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["target"] == "skill:review")
        .filter_map(|i| i["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"unreadable-upstream"), "{v}");
    assert!(!kinds.contains(&"removed-upstream"), "{v}");
}
