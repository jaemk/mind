//! The `tool` item kind (spec/tooling.md).

use super::*;

#[test]
fn path_token_outside_markdown_does_not_expand() {
    // spec: TOOL-19
    // Path tokens follow the same markdown-only expansion rule as `{{ns:}}`
    // (NS-53): `{{self}}` in the skill's SKILL.md expands, but the same token in
    // a bundled non-markdown script is left exactly as written.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff for bugs\n---\nSee {{self}}/run.sh.\n",
    );
    sb.write_and_commit(
        "skills/review/run.sh",
        "#!/bin/sh\n# lives at {{self}}\necho ok\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "review", "--yes"]).success);

    let md = std::fs::read_to_string(sb.mind_home.join("store/skill/review/SKILL.md")).unwrap();
    assert!(
        md.contains("/store/skill/review/run.sh"),
        "the markdown file's {{{{self}}}} token must expand: {md}"
    );
    let script = std::fs::read_to_string(sb.mind_home.join("store/skill/review/run.sh")).unwrap();
    assert!(
        script.contains("{{self}}"),
        "the non-markdown file's token must be left exactly as written: {script}"
    );
}

#[test]
fn example_tooling_expands_path_tokens() {
    // spec: TOOL-3, TOOL-10, TOOL-11, TOOL-12
    // The tooling example ships a `tool` plus a skill that references it through
    // path tokens. Learning the skill pulls in the tool it depends on, and the
    // tokens expand to store paths.
    let sb = Sandbox::from_example("tooling");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    // spec: DEP-8
    // The skill's path tokens name the `detect` tool, so learning the skill
    // pulls the tool in as a dependency (confirmed by `--yes`).
    assert!(sb.mind(&["learn", "scan", "--yes"]).success);
    let skill = std::fs::read_to_string(sb.mind_home.join("store/skill/scan/SKILL.md")).unwrap();
    assert!(
        skill.contains("store/tool/detect/detect.sh"),
        "{{tools:detect}} expands to the tool entrypoint: {skill}"
    );
    assert!(
        skill.contains("store/tool/detect/lib.sh"),
        "{{path:tool:detect}} reaches a non-entrypoint file: {skill}"
    );
    assert!(
        skill.contains("store/skill/scan"),
        "{{self}} expands to the skill's own store dir: {skill}"
    );
    assert!(
        !skill.contains("{{tools:") && !skill.contains("{{self") && !skill.contains("{{path:"),
        "tokens should be gone: {skill}"
    );

    // The tool is store-only: it lands in the store, linked into no agent home.
    assert!(
        sb.mind_home.join("store/tool/detect/detect.sh").exists(),
        "the detect tool should be copied into the store"
    );

    // spec: RES-10 DEP-8
    // The shared resource came along too, and its token names it.
    assert!(
        sb.mind_home.join("store/resource/markers.json").is_file(),
        "the markers.json resource should be copied into the store"
    );
    assert!(
        skill.contains("store/resource/markers.json") && !skill.contains("{{resources:"),
        "{{resources:markers.json}} expands to the resource: {skill}"
    );

    // spec: NS-53 TOOL-19
    // The tool's own non-markdown files mention the tokens that reach them (as
    // documentation), but those tokens do not expand there: markdown is the
    // only place any token family expands.
    let detect_sh =
        std::fs::read_to_string(sb.mind_home.join("store/tool/detect/detect.sh")).unwrap();
    assert!(
        detect_sh.contains("{{tools:detect}}"),
        "a token in a non-markdown file is left exactly as written: {detect_sh}"
    );
    let lib_sh = std::fs::read_to_string(sb.mind_home.join("store/tool/detect/lib.sh")).unwrap();
    assert!(
        lib_sh.contains("{{path:tool:detect}}") && lib_sh.contains("{{tools:detect}}"),
        "a token in a non-markdown file is left exactly as written: {lib_sh}"
    );
}

/// A source with two shared tools and a skill (plus a bundled script) that
/// reference them via path tokens. Committed and ready to meld.
fn tool_source() -> Sandbox {
    let sb = Sandbox::bare("agents");
    // Two shared tools; each entrypoint is the convention default file.
    write(
        &sb.source.join("tools/shard/shard"),
        "#!/bin/sh\necho shard\n",
    );
    // detect's helper file references the other tool (tool -> tool).
    write(
        &sb.source.join("tools/detect/detect"),
        "#!/bin/sh\necho detect\n",
    );
    write(
        &sb.source.join("tools/detect/lib.sh"),
        "exec {{tools:shard}} \"$@\"\n",
    );
    // A skill referencing its own file, a tool's entrypoint, and a non-entrypoint
    // file inside a tool. Its bundled run.sh also calls a tool (script -> tool).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\nrun {{self}}/run.sh\ndetect {{tools:detect}} .\nlib {{path:tool:detect}}/lib.sh\n",
    );
    write(
        &sb.source.join("skills/review/run.sh"),
        "#!/bin/sh\n{{tools:detect}} run\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "tools"]);
    sb
}

#[test]
fn tool_installs_store_only_and_tokens_expand_in_markdown_only() {
    // spec: TOOL-3 TOOL-13 TOOL-14 TOOL-15 TOOL-19 NS-53
    // Path tokens follow the same markdown-only narrowing as `{{ns:}}` (NS-53,
    // TOOL-19): a token in the skill's SKILL.md expands, but the same token
    // family in a bundled non-markdown script (the skill's own `run.sh`, a
    // tool's own `lib.sh`) is left exactly as written.
    let sb = tool_source();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    let store = sb.mind_home.join("store");
    // The tools install to the store...
    assert!(store.join("tool/detect/detect").is_file());
    assert!(store.join("tool/shard/shard").is_file());
    // ...but are store-only: not linked into the agent home.
    assert!(
        !sb.claude_home.join("tools").exists(),
        "a tool must not be linked into an agent home"
    );
    assert!(!sb.claude_home.join("skills/detect").exists());
    // The skill links normally.
    let link = sb.claude_home.join("skills/review");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the skill links as usual"
    );

    // Tokens expanded to store paths in the SKILL.md (markdown)...
    let s = store.display().to_string();
    let skill_md = std::fs::read_to_string(store.join("skill/review/SKILL.md")).unwrap();
    assert!(
        skill_md.contains(&format!("run {s}/skill/review/run.sh")),
        "{skill_md}"
    );
    assert!(
        skill_md.contains(&format!("detect {s}/tool/detect/detect .")),
        "{skill_md}"
    );
    assert!(
        skill_md.contains(&format!("lib {s}/tool/detect/lib.sh")),
        "{skill_md}"
    );
    // ...but NOT in the skill's bundled non-markdown script (NS-53/TOOL-19):
    // the token is left exactly as written.
    let run_sh = std::fs::read_to_string(store.join("skill/review/run.sh")).unwrap();
    assert!(
        run_sh.contains("{{tools:detect}} run"),
        "a non-markdown file's token must not expand: {run_sh}"
    );
    // ...nor in a tool's own non-markdown helper file (tool -> tool, NS-53).
    let lib_sh = std::fs::read_to_string(store.join("tool/detect/lib.sh")).unwrap();
    assert!(
        lib_sh.contains("exec {{tools:shard}} \"$@\""),
        "a non-markdown file's token must not expand: {lib_sh}"
    );
}

#[test]
fn tool_prefix_applies_to_store_and_tokens() {
    // spec: TOOL-6
    let sb = tool_source();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--as", "jk", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let store = sb.mind_home.join("store");
    // The tool installs under the prefixed effective name.
    assert!(store.join("tool/jk:detect/detect").is_file());
    // The same tokens resolve to the prefixed store paths.
    let skill_md = std::fs::read_to_string(store.join("skill/jk:review/SKILL.md")).unwrap();
    assert!(
        skill_md.contains(&format!("{}/tool/jk:detect/detect", store.display())),
        "{skill_md}"
    );
}

#[test]
fn tool_with_explicit_link_is_surfaced() {
    // spec: TOOL-4
    let sb = Sandbox::bare("agents");
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"tool\"\nname = \"detect\"\npath = \"tools/detect\"\nlink = \"agents/detect\"\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "linked-tool"]);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let link = sb.claude_home.join("agents/detect");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "an explicit link surfaces the tool in the agent home"
    );
}
