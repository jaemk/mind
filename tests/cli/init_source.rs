//! `init-source` (spec/init-source.md).

use super::*;

#[test]
fn init_source_reports_refs_scaffolds_toml_and_templates() {
    // spec: INIT-1, INIT-2, INIT-3, INIT-4, INIT-5, INIT-6
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring");
    // skill `review` references agent `dev` in bare prose and `style` via a token.
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\nHand off to dev, then see {{ns:style}}.\n",
    );
    write(
        &repo.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    write(
        &repo.join("rules/style.md"),
        "---\ndescription: style\n---\n# style\n",
    );
    let dir = repo.to_str().unwrap();

    // Report mode: items + reference graph + scaffold; nothing in the store.
    let r = sb.mind(&["init-source", dir]);
    assert!(r.success, "init-source failed: {} {}", r.stdout, r.stderr);
    // INIT-2 / INIT-4: items and references are reported.
    assert!(
        r.stdout.contains("review") && r.stdout.contains("dev") && r.stdout.contains("style"),
        "items and references must be reported: {}",
        r.stdout
    );
    // INIT-9: this fixture declares no prefix, so the bare `dev` mention is NOT
    // flagged (an unprefixed source's bare references resolve as written). The
    // prefix-gated advisory is covered in tests/item_lifecycle.rs.
    assert!(
        !r.stdout.contains("advisory [unguarded-reference]"),
        "no prefix => no unguarded-reference advisory (INIT-9): {}",
        r.stdout
    );
    // INIT-3: a mind.toml is scaffolded when absent, with a `[source]` table and
    // a commented-out generic namespace example whose value matches its comment.
    let scaffold = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        scaffold.contains("[source]") && scaffold.contains("# namespace = \"prefix\""),
        "scaffold must carry a [source] table and a generic commented namespace: {scaffold}"
    );
    // INIT-6: init-source registers nothing (no store state).
    assert!(
        !sb.mind_home.join("sources.json").exists(),
        "init-source must not write to the store"
    );

    // INIT-3: an existing mind.toml is left unchanged on a re-run.
    let toml_before = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(sb.mind(&["init-source", dir]).success);
    assert_eq!(
        std::fs::read_to_string(repo.join("mind.toml")).unwrap(),
        toml_before,
        "an existing mind.toml must not be overwritten"
    );

    // INIT-5: --template wraps the bare `dev`; the existing `{{ns:style}}` survives.
    let t = sb.mind(&["init-source", dir, "--template"]);
    assert!(
        t.success,
        "init-source --template failed: {} {}",
        t.stdout, t.stderr
    );
    let review = std::fs::read_to_string(repo.join("skills/review/SKILL.md")).unwrap();
    assert!(
        review.contains("{{ns:dev}}"),
        "the bare `dev` reference must be templated: {review}"
    );
    assert!(
        review.contains("{{ns:style}}"),
        "the existing token must survive: {review}"
    );
    assert!(
        !review.contains("to dev,"),
        "the bare `dev` must be replaced, not duplicated: {review}"
    );
}

#[test]
fn init_source_template_skips_a_bare_mention_in_a_non_markdown_file() {
    // spec: INIT-5
    // --template's file gate is `namespace::is_markdown` (the same extension
    // set install expands tokens in, NS-53), not an exact-`.md` test: a bare
    // sibling mention in a `.sh` file is left alone (install would never
    // expand a token there either), while the SAME mention in a `.markdown`
    // file (a recognized-but-not-`.md` extension) IS templated.
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring-md-gate");
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo.join("skills/review/run.sh"),
        "#!/bin/sh\n# hand off to dev\necho dev\n",
    );
    write(
        &repo.join("skills/review/notes.markdown"),
        "hand off to dev when done\n",
    );
    write(
        &repo.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let dir = repo.to_str().unwrap();

    let t = sb.mind(&["init-source", dir, "--template"]);
    assert!(
        t.success,
        "init-source --template failed: {} {}",
        t.stdout, t.stderr
    );

    let script = std::fs::read_to_string(repo.join("skills/review/run.sh")).unwrap();
    assert!(
        script.contains("hand off to dev") && !script.contains("{{ns:dev}}"),
        "a bare mention in a non-markdown file must not be templated: {script}"
    );

    let notes = std::fs::read_to_string(repo.join("skills/review/notes.markdown")).unwrap();
    assert!(
        notes.contains("{{ns:dev}}"),
        "a bare mention in a recognized non-`.md` markdown extension (.markdown) \
         must still be templated: {notes}"
    );
}

/// The workflow carve-out INIT-5 now spells out: `--template`'s gate is
/// `namespace::is_markdown`, which is NARROWER than the set install expands
/// (install also expands a workflow's `.js`, WF-25). So a bare sibling mention
/// inside a workflow's JavaScript is left alone, for the same reason `review
/// --fix` leaves one alone (NS-54): word-boundary rewriting inside code can
/// corrupt working code, and a workflow file is code.
///
/// Without this, the obvious "consistency fix" -- pointing `--template` at the
/// item-aware gate, since install expands a workflow's `.js` -- breaks nothing
/// any test asserts, while silently rewriting identifiers in a source's
/// JavaScript. The `.md` half is the control: the same mention in the same
/// source IS templated, so this pins a deliberate asymmetry rather than an
/// inert file walk.
#[test]
fn init_source_template_leaves_a_workflows_js_alone() {
    // spec: INIT-5 NS-54 WF-25
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring-workflow-gate");
    let js = "export const meta = {\n  name: 'ship-it',\n  description: 'ship',\n}\n\
              // hand off to dev when the build is green\nagent('ask dev to review')\n";
    write(&repo.join("workflows/ship-it.js"), js);
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\nhand off to dev when done\n",
    );
    write(
        &repo.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let dir = repo.to_str().unwrap();

    let t = sb.mind(&["init-source", dir, "--template"]);
    assert!(
        t.success,
        "init-source --template failed: {} {}",
        t.stdout, t.stderr
    );

    let after = std::fs::read_to_string(repo.join("workflows/ship-it.js")).unwrap();
    assert_eq!(
        after, js,
        "--template must leave a workflow's .js byte-identical (INIT-5), even \
         though install DOES expand tokens there (WF-25)"
    );

    // Control: markdown in the same source, same mention, IS templated -- so the
    // untouched `.js` above is the gate at work, not a no-op run.
    let skill = std::fs::read_to_string(repo.join("skills/review/SKILL.md")).unwrap();
    assert!(
        skill.contains("{{ns:dev}}"),
        "the markdown control must be templated: {skill}"
    );
}

#[test]
fn init_source_flags_helper_script_duplicated_across_items() {
    // spec: INIT-7
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring");
    write(
        &repo.join("skills/a/SKILL.md"),
        "---\ndescription: a\n---\n# a\n",
    );
    write(&repo.join("skills/a/helper.sh"), "#!/bin/sh\necho shared\n");
    write(
        &repo.join("skills/b/SKILL.md"),
        "---\ndescription: b\n---\n# b\n",
    );
    write(&repo.join("skills/b/helper.sh"), "#!/bin/sh\necho shared\n");
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir]);
    assert!(r.success, "init-source failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("advisory [duplicate-tooling]") && r.stdout.contains("helper.sh"),
        "init-source must surface the duplicate-tooling advisory like review: {}",
        r.stdout
    );
}

#[test]
fn init_source_marketplace_generates_correct_json() {
    // spec: INIT-10 INIT-11
    // Conventional source with no mind.toml. --marketplace generates a
    // marketplace.json with name = dir basename, source = ".", no `skills` key.
    let sb = Sandbox::new();
    let repo = sb.base.join("my-plugin");
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(
        r.success,
        "init-source --marketplace failed: {} {}",
        r.stdout, r.stderr
    );

    let mkt_path = repo.join(".claude-plugin/marketplace.json");
    assert!(
        mkt_path.exists(),
        ".claude-plugin/marketplace.json must be created"
    );
    let content = std::fs::read_to_string(&mkt_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");

    // INIT-11: name defaults to dir basename when no prefix or --namespace.
    assert_eq!(
        v["name"], "my-plugin",
        "top-level name must be the dir basename"
    );
    let plugin = &v["plugins"][0];
    assert_eq!(plugin["name"], "my-plugin", "plugin entry name");
    assert_eq!(plugin["source"], ".", "plugin source must be '.'");

    // Without --flat-skills the skills key must be absent (INIT-10).
    assert!(
        plugin.get("skills").is_none(),
        "skills key must be absent without --flat-skills: {content}"
    );
}

#[test]
fn init_source_marketplace_uses_mindtoml_prefix() {
    // spec: INIT-10 INIT-11
    // When mind.toml has [source].prefix, that value becomes the plugin name.
    let sb = Sandbox::new();
    let repo = sb.base.join("irrelevant-dirname");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    write(
        &repo.join("mind.toml"),
        "[source]\nprefix = \"mypkg\"\ndescription = \"A great plugin\"\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(
        v["name"], "mypkg",
        "prefix from mind.toml must override dirname"
    );
    assert_eq!(v["plugins"][0]["name"], "mypkg", "plugin entry name");
    assert_eq!(
        v["plugins"][0]["description"], "A great plugin",
        "description from mind.toml [source].description"
    );
}

#[test]
fn init_source_marketplace_namespace_flag_overrides() {
    // spec: INIT-10 INIT-11 INIT-12
    // --namespace <n> overrides both mind.toml prefix and dir basename; the value
    // is also written as [source].prefix in mind.toml.
    let sb = Sandbox::new();
    let repo = sb.base.join("some-dir");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace", "--namespace", "foo"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    // marketplace.json uses the --namespace value.
    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(v["name"], "foo", "name must be the --namespace value");
    assert_eq!(v["plugins"][0]["name"], "foo", "plugin entry name");

    // mind.toml must carry namespace = "foo" (INIT-11, DSC-82 write key).
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"foo\""),
        "mind.toml must carry namespace = \"foo\": {toml}"
    );
}

#[test]
fn init_source_marketplace_no_overwrite() {
    // spec: INIT-10
    // An existing .claude-plugin/marketplace.json is left unchanged; the command
    // still exits 0 and prints the "already exists" notice.
    let sb = Sandbox::new();
    let repo = sb.base.join("existing-mkt");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let sentinel = r#"{"name":"sentinel","plugins":[]}"#;
    write(&repo.join(".claude-plugin/marketplace.json"), sentinel);
    let dir = repo.to_str().unwrap();

    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(
        r.success,
        "must exit 0 even when file exists: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("already exists"),
        "must print 'already exists' message: {}",
        r.stdout
    );

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    assert_eq!(content, sentinel, "file must be left unchanged");
}

#[test]
fn init_source_flat_skills_sets_mindtoml_key() {
    // spec: INIT-12
    // Three sub-cases: no mind.toml, existing without the key, existing with it.

    let sb = Sandbox::new();

    // Case A: no existing mind.toml -> creates from scaffold with flat-skills = true.
    let repo_a = sb.base.join("flat-a");
    write(
        &repo_a.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    let r = sb.mind(&["init-source", repo_a.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let toml_a = std::fs::read_to_string(repo_a.join("mind.toml")).unwrap();
    assert!(
        toml_a.contains("flat-skills = true"),
        "case A: flat-skills must be set: {toml_a}"
    );

    // Case B: existing mind.toml without flat-skills -> inserts the key.
    let repo_b = sb.base.join("flat-b");
    write(
        &repo_b.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo_b.join("mind.toml"),
        "[source]\ndescription = \"existing\"\n",
    );
    let r = sb.mind(&["init-source", repo_b.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let toml_b = std::fs::read_to_string(repo_b.join("mind.toml")).unwrap();
    assert!(
        toml_b.contains("flat-skills = true"),
        "case B: flat-skills must be inserted: {toml_b}"
    );
    assert!(
        toml_b.contains("description = \"existing\""),
        "case B: existing content must be preserved: {toml_b}"
    );

    // Case C: existing mind.toml with flat-skills = false -> replaces with true.
    let repo_c = sb.base.join("flat-c");
    write(
        &repo_c.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo_c.join("mind.toml"),
        "[source]\nflat-skills = false\ndescription = \"existing\"\n",
    );
    let r = sb.mind(&["init-source", repo_c.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case C failed: {} {}", r.stdout, r.stderr);
    let toml_c = std::fs::read_to_string(repo_c.join("mind.toml")).unwrap();
    assert!(
        toml_c.contains("flat-skills = true"),
        "case C: flat-skills must be true: {toml_c}"
    );
    assert!(
        !toml_c.contains("flat-skills = false"),
        "case C: old value must be gone: {toml_c}"
    );
}

#[test]
fn init_source_flat_skills_marketplace_skills_array() {
    // spec: INIT-10 INIT-12
    // With --flat-skills --marketplace on a flat source (skill dirs at root),
    // the generated marketplace.json includes a `skills` array with the relative
    // dir path of each discovered skill.
    let sb = Sandbox::new();
    let repo = sb.base.join("flat-mkt");
    // Two flat skills at the repo root.
    write(
        &repo.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo.join("build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--flat-skills", "--marketplace"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    let skills = v["plugins"][0]["skills"]
        .as_array()
        .expect("skills array must be present with --flat-skills");

    let skill_strings: Vec<&str> = skills.iter().filter_map(|s| s.as_str()).collect();
    assert!(
        skill_strings.contains(&"review"),
        "skills must include 'review': {skill_strings:?}"
    );
    assert!(
        skill_strings.contains(&"build"),
        "skills must include 'build': {skill_strings:?}"
    );

    // mind.toml must have flat-skills = true.
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("flat-skills = true"),
        "mind.toml must have flat-skills = true: {toml}"
    );
}

#[test]
fn init_source_namespace_writes_namespace_key() {
    // spec: INIT-11 INIT-12
    // --namespace bar writes namespace = "bar" to mind.toml (creating it if
    // absent; DSC-82 write key).
    let sb = Sandbox::new();
    let repo = sb.base.join("ns-test");
    write(
        &repo.join("skills/scan/SKILL.md"),
        "---\ndescription: scan\n---\n# scan\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--namespace", "bar"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"bar\""),
        "mind.toml must have namespace = \"bar\": {toml}"
    );
    // Other scaffold content is preserved.
    assert!(
        toml.contains("[source]"),
        "must still have [source] header: {toml}"
    );
}

#[test]
fn init_source_marketplace_placeholder_description() {
    // spec: INIT-10
    // When no [source].description is present in mind.toml (or no mind.toml
    // exists at all), the generated marketplace.json must contain the placeholder
    // description "TODO: describe this plugin", not an empty string.
    let sb = Sandbox::new();

    // Case A: no mind.toml at all.
    let repo_a = sb.base.join("placeholder-a");
    write(
        &repo_a.join("skills/plan/SKILL.md"),
        "---\ndescription: plan\n---\n# plan\n",
    );
    let r = sb.mind(&["init-source", repo_a.to_str().unwrap(), "--marketplace"]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let content_a =
        std::fs::read_to_string(repo_a.join(".claude-plugin/marketplace.json")).unwrap();
    let v_a: serde_json::Value = serde_json::from_str(&content_a).expect("valid JSON");
    assert_eq!(
        v_a["plugins"][0]["description"], "TODO: describe this plugin",
        "case A: description must be the placeholder when no mind.toml: {content_a}"
    );

    // Case B: mind.toml exists but [source].description is empty.
    let repo_b = sb.base.join("placeholder-b");
    write(
        &repo_b.join("skills/plan/SKILL.md"),
        "---\ndescription: plan\n---\n# plan\n",
    );
    write(&repo_b.join("mind.toml"), "[source]\ndescription = \"\"\n");
    let r = sb.mind(&["init-source", repo_b.to_str().unwrap(), "--marketplace"]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let content_b =
        std::fs::read_to_string(repo_b.join(".claude-plugin/marketplace.json")).unwrap();
    let v_b: serde_json::Value = serde_json::from_str(&content_b).expect("valid JSON");
    assert_eq!(
        v_b["plugins"][0]["description"], "TODO: describe this plugin",
        "case B: description must be the placeholder when description is empty: {content_b}"
    );
}

#[test]
fn init_source_namespace_updates_existing_mindtoml() {
    // spec: INIT-11
    // When --namespace is passed and mind.toml already exists, the prefix key is
    // inserted or updated in the existing file without destroying its other content.
    let sb = Sandbox::new();

    // Case A: existing mind.toml without a prefix key -- prefix is inserted.
    let repo_a = sb.base.join("ns-existing-a");
    write(
        &repo_a.join("skills/deploy/SKILL.md"),
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    write(
        &repo_a.join("mind.toml"),
        "[source]\ndescription = \"my source\"\n",
    );
    let r = sb.mind(&[
        "init-source",
        repo_a.to_str().unwrap(),
        "--namespace",
        "mypkg",
    ]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let toml_a = std::fs::read_to_string(repo_a.join("mind.toml")).unwrap();
    assert!(
        toml_a.contains("namespace = \"mypkg\""),
        "case A: namespace must be inserted: {toml_a}"
    );
    assert!(
        toml_a.contains("description = \"my source\""),
        "case A: existing content must be preserved: {toml_a}"
    );

    // Case B: existing mind.toml with a different prefix -- prefix is replaced.
    let repo_b = sb.base.join("ns-existing-b");
    write(
        &repo_b.join("skills/deploy/SKILL.md"),
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    write(
        &repo_b.join("mind.toml"),
        "[source]\nprefix = \"old\"\ndescription = \"my source\"\n",
    );
    let r = sb.mind(&[
        "init-source",
        repo_b.to_str().unwrap(),
        "--namespace",
        "new",
    ]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let toml_b = std::fs::read_to_string(repo_b.join("mind.toml")).unwrap();
    assert!(
        toml_b.contains("namespace = \"new\""),
        "case B: namespace must be updated to new: {toml_b}"
    );
    assert!(
        !toml_b.contains("prefix = \"old\""),
        "case B: old prefix key must be gone: {toml_b}"
    );
    assert!(
        toml_b.contains("description = \"my source\""),
        "case B: other content must be preserved: {toml_b}"
    );
}

#[test]
fn init_source_namespace_overrides_existing_prefix_in_marketplace() {
    // spec: INIT-10 INIT-11
    // When mind.toml already has [source].prefix = "old" but --namespace "new" is
    // passed, the generated marketplace.json uses "new" as the plugin name
    // (--namespace beats [source].prefix, INIT-11) and mind.toml is updated so
    // the stored prefix also becomes "new".
    let sb = Sandbox::new();
    let repo = sb.base.join("ns-override");
    write(
        &repo.join("skills/lint/SKILL.md"),
        "---\ndescription: lint\n---\n# lint\n",
    );
    write(
        &repo.join("mind.toml"),
        "[source]\nprefix = \"old\"\ndescription = \"lint tools\"\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace", "--namespace", "new"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    // marketplace.json must use "new", not "old" or the dirname.
    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(
        v["name"], "new",
        "top-level name must be the --namespace value: {content}"
    );
    assert_eq!(
        v["plugins"][0]["name"], "new",
        "plugin entry name must be --namespace value: {content}"
    );
    // description comes from existing mind.toml [source].description.
    assert_eq!(
        v["plugins"][0]["description"], "lint tools",
        "description from existing mind.toml must be preserved: {content}"
    );

    // mind.toml must have the namespace updated to "new" (DSC-82 write key).
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"new\""),
        "mind.toml namespace must be updated to 'new': {toml}"
    );
    assert!(
        !toml.contains("prefix = \"old\""),
        "old prefix key must no longer be present: {toml}"
    );
}

// ---------------------------------------------------------------------------
// H1 / NS-43 / NS-45: --yes flag forces non-interactive collision path
// ---------------------------------------------------------------------------
