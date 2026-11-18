//! `mind init-source`: the maintainer scaffolder.

use super::*;

/// `mind init-source [path] [--template]` — maintainer scaffolding. Discovers the
/// repo's items, reports the intra-source reference graph, scaffolds a `mind.toml`
/// if absent, and (with `--template`) rewrites bare sibling references into
/// `{{ns:}}` tokens. With `--marketplace` generates `.claude-plugin/marketplace.json`.
/// Operates only on the target directory: no store, no agent home, no network (INIT-6).
// spec: INIT-1 INIT-2 INIT-3 INIT-4 INIT-6 INIT-9 INIT-10 INIT-11 INIT-12
pub fn init_source(
    dir: Option<&str>,
    template: bool,
    marketplace: bool,
    flat_skills_flag: bool,
    namespace: Option<String>,
) -> Result<()> {
    let dir = dir.unwrap_or(".");
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        return Err(MindError::NotADirectory {
            path: dir.to_string(),
        });
    }
    let root = path.canonicalize().map_err(|e| MindError::io(path, e))?;

    let toml_path = root.join("mind.toml");

    // Read the pre-existing mind.toml content (if any) for the scaffold patching
    // step and for extracting description/prefix for the marketplace manifest.
    let pre_toml = read_existing_mind_toml(&toml_path, crate::error::metadata_size_limit())?;

    // Discover items exactly as melding would (INIT-2): build a local Source for
    // the directory and scan it (honors convention + mind.toml + min-mind-version).
    // Set flat_skills on the source struct before scanning so flat layout is used
    // even before mind.toml is written (INIT-12, DSC-74).
    let mut source = parse_spec(&root.to_string_lossy())?;
    if flat_skills_flag {
        source.flat_skills = true;
    }
    let mut items: Vec<CatalogItem> = Vec::new();
    catalog::scan_source_at(&root, &source, &mut items)?;

    println!("init-source: {}", root.display());
    if items.is_empty() {
        println!(
            "  no items found (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md, \
             commands/<name>.md, workflows/<name>.js, tools/<name>/)"
        );
    } else {
        println!("  {} item(s):", items.len());
        for it in &items {
            println!("    {} {}", it.kind, it.display_name());
        }
    }

    // Reference graph (INIT-4): per item, the siblings it references via tokens
    // (informational) and the ones it mentions in bare prose. The bare mentions
    // are reported as `unguarded-reference` advisories in the same format as
    // `review` (CLI-131), so review and init-source read identically.
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    // INIT-9: the bare-prose `unguarded-reference` advisory fires only when an
    // effective prefix is in force (matching `meld` NS-23 and `review` CLI-133):
    // absent a prefix, bare references resolve as written. The `{{ns:}}`-token
    // graph and the `--template` rewrite below are unaffected by this gate.
    let prefix_in_force = items.iter().any(|it| it.prefix.is_some());
    let mut findings: Vec<crate::review::Finding> = Vec::new();
    for it in &items {
        let content = read_item_text(it);
        let tokens: Vec<String> = crate::namespace::referenced_names(&content)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        let bare: Vec<String> = crate::namespace::unguarded_refs(&content, &siblings)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        if !tokens.is_empty() {
            println!(
                "  {} {} -> {} (tokenized)",
                it.kind,
                it.name,
                tokens.join(", ")
            );
        }
        if prefix_in_force && !bare.is_empty() {
            findings.push(crate::review::Finding::advisory(
                "unguarded-reference",
                format!(
                    "{}: references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                    it.key().as_str(),
                    bare.join(", ")
                ),
            ));
        }
    }
    // The `--template` hint applies only to bare prose references; duplicate
    // tooling is structural and not something templating fixes.
    let has_unguarded = findings.iter().any(|f| f.kind == "unguarded-reference");
    // INIT-7: surface the same duplicate-tooling advisories `review` reports
    // (CLI-144), so the two commands read identically here too.
    findings.extend(crate::review::duplicate_tooling_findings(&items));
    crate::review::print_findings(&[], &findings);
    if has_unguarded && !template {
        // spec: CLI-225 -- `dir` is the user's own CLI path argument, not
        // restricted against shell metacharacters, and lands in a pasteable
        // `mind init-source <dir> --template` remedy, so it is shell-quoted
        // before printing (same rule as source/item identities elsewhere).
        let quoted_dir = crate::error::shell_quote(dir);
        println!(
            "run `mind init-source {quoted_dir} --template` to wrap the bare references as {{{{ns:name}}}}"
        );
    }

    // spec: VER-131 -- a generated mind.toml declares the default release-tag
    // template when the repo's own tags already include releases under it.
    // The tags are read from the local repo, so this stays offline (INIT-6).
    let release_tags = |text: String| -> String {
        let template = crate::version::Template::parse(crate::scaffold::RELEASE_TAGS)
            .expect("the default template parses");
        let found = git::ls_remote_tags(&root.to_string_lossy())
            .map(|tags| crate::version::releases(&template, &tags).0.len())
            .unwrap_or(0);
        if found == 0 {
            return text;
        }
        println!(
            "  {found} release tag(s) match {}; declaring release-tags",
            crate::scaffold::RELEASE_TAGS
        );
        crate::scaffold::with_release_tags(&text)
    };

    // mind.toml handling: INIT-3 (create if absent) + INIT-12 (patch when flags
    // require it). When flat_skills_flag or namespace is set, always write (patch
    // existing or create with scaffold); otherwise keep the create-if-absent logic.
    if flat_skills_flag || namespace.is_some() {
        let mut patched = crate::scaffold::patch_source_meta(
            pre_toml.as_deref(),
            flat_skills_flag,
            namespace.as_deref(),
        );
        if pre_toml.is_none() {
            patched = release_tags(patched);
        }
        std::fs::write(&toml_path, &patched).map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
    } else if toml_path.exists() {
        println!("  mind.toml already exists; left unchanged");
    } else {
        std::fs::write(&toml_path, release_tags(crate::scaffold::scaffold()))
            .map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
    }

    // Marketplace manifest (INIT-10): generate .claude-plugin/marketplace.json
    // when --marketplace is passed, unless one already exists.
    if marketplace {
        if plugin_manifest::find_marketplace_manifest(&root).is_some() {
            println!("  .claude-plugin/marketplace.json already exists; left unchanged");
        } else {
            // INIT-11: name priority: --namespace > mind.toml prefix > dir basename.
            let dir_basename = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Read the current mind.toml (after any writes) for the prefix.
            let mindfile_prefix =
                crate::mindfile::MindToml::load(&root)?.and_then(|mt| mt.source.prefix);
            let name = crate::scaffold::plugin_name(
                &dir_basename,
                mindfile_prefix.as_deref(),
                namespace.as_deref(),
            );

            // Description: from [source].description in the pre-existing mind.toml
            // (preserved through patch), else placeholder.
            let description = crate::mindfile::MindToml::load(&root)
                .ok()
                .flatten()
                .and_then(|mt| mt.source.description)
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| "TODO: describe this plugin".to_string());

            // Skills array only when --flat-skills is also in effect (INIT-10).
            let skills: Option<Vec<String>> = if flat_skills_flag {
                let mut paths: Vec<String> = items
                    .iter()
                    .filter(|it| it.kind == ItemKind::Skill)
                    .filter_map(|it| {
                        it.path
                            .strip_prefix(&root)
                            .ok()
                            .map(|rel| rel.to_string_lossy().into_owned())
                    })
                    .collect();
                paths.sort();
                Some(paths)
            } else {
                None
            };

            let plugin_dir = root.join(".claude-plugin");
            std::fs::create_dir_all(&plugin_dir).map_err(|e| MindError::io(&plugin_dir, e))?;
            let json_path = plugin_manifest::marketplace_manifest_path(&root);
            let json =
                crate::scaffold::render_marketplace_json(&name, &description, skills.as_deref());
            std::fs::write(&json_path, &json).map_err(|e| MindError::io(&json_path, e))?;
            println!("  wrote .claude-plugin/marketplace.json");
        }
    }

    // Templating (INIT-5): rewrite bare sibling mentions to tokens, per file.
    if template {
        let mut total = 0usize;
        for it in &items {
            // Exclude the item's own name so a self-mention is not wrapped.
            let mut sibs = siblings.clone();
            sibs.remove(&it.name);
            for file in crate::review::item_files(it) {
                // {{ns:}} is a prose reference (NS-24); only markdown carries
                // prose. Never templatize scripts/data, where every word is code.
                //
                // spec: INIT-5 NS-54 -- the gate here is prose-ness
                // (`namespace::is_markdown`, the markdown extension set rather
                // than an exact-`.md` test), NOT the set install expands. The
                // two differ: install also expands a workflow's `.js` (WF-25)
                // and an `expand:`-listed script (NS-57), and this pass must
                // still leave both alone. It rewrites bare prose into tokens by
                // matching sibling names as words, and in code a sibling name
                // may be an identifier, a key, or part of one, so a rewrite
                // there is as likely to corrupt the file as to improve it.
                if !crate::namespace::is_markdown(&file) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&file) else {
                    continue; // skip non-UTF-8 / unreadable files
                };
                let (rewritten, n) = crate::namespace::templatize(&content, &sibs);
                if n > 0 {
                    std::fs::write(&file, &rewritten).map_err(|e| MindError::io(&file, e))?;
                    println!("  templated {n} reference(s) in {}", file.display());
                    total += n;
                }
            }
        }
        if total == 0 {
            println!("  no bare references to template");
        }
    }
    Ok(())
}

/// Read all of an item's token-expanding text files into one buffer, for
/// `{{ns:}}` dependency-edge detection (DEP-1). Narrowed to the files install
/// expands (`namespace::item_expands_tokens`, NS-53/NS-57/WF-25) so a `{{ns:}}`
/// token in a file install never expands -- and so never treats as a dependency
/// -- does not create a phantom dependency edge here either.
///
/// It has to be the ITEM-aware gate, not the extension-and-kind half: install
/// expands an `expand:`-listed file too (NS-57), so reading the narrower set
/// dropped the real dependency edges a token in one of those files declares.
///
/// spec: DSC-90 WF-55 -- content semantics, the same as install, which copies
/// and expands the whole file: the read is NOT capped, so an over-cap
/// workflow's `{{ns:}}` tokens still pull their referents into the closure.
/// The unbounded-read hazard (a device, fifo, or symlink out of the clone) is
/// closed by the catalog's no-follow regular-file guarantee (DSC-108) and, as
/// defense in depth, by [`is_regular_file_nofollow`] here.
pub(super) fn read_item_text(item: &CatalogItem) -> String {
    let mut buf = String::new();
    for file in crate::review::item_files(item) {
        if !crate::namespace::item_expands_tokens(item, &item.path, &file) {
            continue;
        }
        if !is_regular_file_nofollow(&file) {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&file) {
            buf.push_str(&content);
            buf.push('\n');
        }
    }
    buf
}

/// `init-source`'s pre-read of an existing `mind.toml`: `None` when there is
/// none, else its text through the metadata cap (DSC-91), so an oversized file
/// is refused with `MetadataTooLarge` before anything is scaffolded.
fn read_existing_mind_toml(path: &std::path::Path, limit: u64) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    crate::error::read_capped_metadata_with(path, limit).map(Some)
}

/// Whether `file` is a regular file, judged WITHOUT following a final
/// symlink: the guard every full-content read of source files goes through, so
/// a symlink, fifo, socket, or device never reaches `read_to_string`
/// (DSC-108 defense in depth).
pub(super) fn is_regular_file_nofollow(file: &std::path::Path) -> bool {
    std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    /// `init-source`'s pre-read of an existing `mind.toml` goes through the
    /// metadata cap: over the cap is `MetadataTooLarge`, under it is the text,
    /// and an absent file is `None`. This pins the pre-read itself, which the
    /// CLI test cannot isolate (the scan's own load would refuse the file too).
    // spec: DSC-91
    #[test]
    fn init_source_pre_read_of_mind_toml_is_capped() {
        let dir = scratch("init-pre-read");
        let toml = dir.join("mind.toml");
        assert!(matches!(read_existing_mind_toml(&toml, 8), Ok(None)));
        let body = "[source]\ndescription = \"forty bytes!!!\"\n";
        assert_eq!(body.len(), 40);
        std::fs::write(&toml, body).unwrap();
        let err = read_existing_mind_toml(&toml, 8).expect_err("over the cap must be refused");
        assert!(matches!(err, MindError::MetadataTooLarge { .. }), "{err:?}");
        assert_eq!(
            read_existing_mind_toml(&toml, crate::error::METADATA_SIZE_LIMIT)
                .unwrap()
                .as_deref(),
            Some(body)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: HOOK-57 HOOK-123
    // The init-source scaffold must include commented [[hooks]] examples for the
    // install, update, and uninstall events, at least one marked optional = true,
    // and must state the install hook's idempotence expectation.
    #[test]
    fn init_source_scaffold_includes_hooks_examples() {
        // Create a temp directory to run init_source in.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp =
            std::env::temp_dir().join(format!("mind-cmd-init-hooks-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        struct Rm(std::path::PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _rm = Rm(tmp.clone());

        // init_source should create the scaffold.
        init_source(Some(tmp.to_str().unwrap()), false, false, false, None)
            .expect("init_source should succeed");

        let toml_path = tmp.join("mind.toml");
        assert!(toml_path.exists(), "mind.toml must be created");
        let contents = std::fs::read_to_string(&toml_path).unwrap();

        // HOOK-57: must include [[hooks]] commented examples.
        assert!(
            contents.contains("[[hooks]]"),
            "scaffold must include a commented [[hooks]] example: {contents}"
        );
        // Must show an install event.
        assert!(
            contents.contains("install"),
            "scaffold must show event = \"install\": {contents}"
        );
        // Must show an uninstall event.
        assert!(
            contents.contains("uninstall"),
            "scaffold must show event = \"uninstall\": {contents}"
        );
        // At least one example marked optional = true.
        assert!(
            contents.contains("optional = true"),
            "scaffold must have at least one optional = true example: {contents}"
        );
        // spec: HOOK-123 -- the scaffold states the idempotence expectation an
        // install hook's re-run at upgrade creates, and shows the update event
        // as the escape for a step that cannot be made idempotent.
        assert!(
            contents.contains("idempotent"),
            "scaffold must state that an install hook is expected to be \
             idempotent: {contents}"
        );
        assert!(
            contents.contains("event = \"update\""),
            "scaffold must show an update-event example: {contents}"
        );
        // spec: HOOK-197 -- an items-event example that says the hook gets the
        // full installed set and should make the host match it.
        assert!(
            contents.contains("# event = \"items\"")
                && contents.contains("MIND_ITEMS_FILE")
                && contents.contains("changes nothing"),
            "scaffold must show an items-event example: {contents}"
        );
        // The hook examples must parse once uncommented: keep each commented
        // `[[hooks]]` header and `key = value` line, without its `# `.
        let uncommented: String = contents
            .lines()
            .skip_while(|l| l.trim() != "# [[hooks]]")
            .filter_map(|l| l.strip_prefix("# "))
            .filter(|l| {
                *l == "[[hooks]]"
                    || l.split_once(" = ").is_some_and(|(k, _)| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: MindToml = toml::from_str(&uncommented)
            .unwrap_or_else(|e| panic!("uncommented scaffold must parse: {e}\n{uncommented}"));
        let events: Vec<HookEvent> = parsed
            .resolved_hooks(std::path::Path::new("mind.toml"))
            .expect("uncommented hooks resolve")
            .into_iter()
            .map(|h| h.event)
            .collect();
        assert!(events.contains(&HookEvent::Items), "{events:?}");
        // All [[hooks]] content is commented out (lines starting with # after trimming).
        // Check that the literal text "[[hooks]]" is preceded by a #.
        let has_uncommented_hooks = contents.lines().any(|l| l.trim() == "[[hooks]]");
        assert!(
            !has_uncommented_hooks,
            "[[hooks]] examples must all be commented out: {contents}"
        );

        // Existing assertions still pass (regression guard).
        assert!(
            contents.contains("[source]"),
            "scaffold must still have [source]"
        );
        assert!(
            contents.contains("# namespace = \"prefix\""),
            "scaffold must still have commented namespace: {contents}"
        );
    }
}
