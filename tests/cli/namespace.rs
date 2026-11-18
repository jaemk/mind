//! Source prefixes and `{{ns:}}` references (spec/namespacing.md).

use super::*;

#[test]
fn mind_toml_prefix_auto_applies_and_alias_overrides() {
    // spec: NS-1, DSC-35
    // Author-declared prefix.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"ag\"\n");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:ag:review"), "{}", probe.stdout);

    // Consumer --as overrides the author's prefix.
    let sb2 = Sandbox::new();
    sb2.write_and_commit("mind.toml", "[source]\nprefix = \"ag\"\n");
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2, "--as", "zz"]).success);
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("skill:zz:review"),
        "{}",
        probe2.stdout
    );
    assert!(!probe2.stdout.contains("ag:review"), "{}", probe2.stdout);
}

#[test]
fn ns_token_expands_to_prefixed_reference_on_install() {
    // spec: NS-11, NS-42
    // A skill sibling token expands prefixed; an agent sibling token expands bare
    // (NS-42: agents link under the bare name regardless of prefix).
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        // {{ns:dev}} is an agent sibling -> expands bare even under prefix.
        "---\nname: lead\ndescription: lead\n---\nDelegate to the {{ns:dev}} agent.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    // `lead` references {{ns:dev}}; confirm the closure with --yes.
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);

    // Store path still uses the effective (prefixed) name; only the link is bare.
    let store = sb.mind_home.join("store/agent/jk:lead");
    let body = std::fs::read_to_string(&store).expect("installed agent file");
    // Agent referent expands bare, not jk:dev.
    assert!(
        body.contains("the dev agent"),
        "expected bare agent ref: {body}"
    );
    assert!(!body.contains("{{ns:dev}}"), "token should be gone: {body}");
    // NS-40: the link is under the bare harness name, not the prefixed name.
    assert!(
        sb.claude_home.join("agents/lead.md").exists(),
        "agent should link as agents/lead.md"
    );
}

#[test]
fn ns_token_expands_to_bare_reference_without_prefix() {
    // spec: NS-14
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the {{ns:dev}} agent.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "lead", "--yes"]).success);

    let body = std::fs::read_to_string(sb.mind_home.join("store/agent/lead")).unwrap();
    assert!(body.contains("the dev agent"), "{body}");
    assert!(!body.contains("{{ns:"), "{body}");
}

#[test]
fn bad_ns_reference_errors_on_install() {
    // spec: NS-12
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nsee {{ns:ghost}}\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    let r = sb.mind(&["learn", "lead"]);
    assert!(!r.success);
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);
}

#[test]
fn bad_ns_token_in_non_markdown_file_installs_clean_but_still_fails_in_markdown() {
    // spec: NS-53
    // Token expansion is narrowed to markdown files (NS-53): a `{{ns:}}` token
    // in a non-markdown bundled file is left exactly as written at install, and
    // an unresolvable one there no longer fails the install -- this retires the
    // BadReference NS-11/NS-12 used to raise for it before the narrowing. The
    // same bad token in the item's own SKILL.md still fails, proving the
    // narrowing is markdown-only, not a blanket relaxation.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/review/run.sh",
        "#!/bin/sh\n# hand off to {{ns:ghost}}\necho ok\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    let r = sb.mind(&["learn", "review", "--yes"]);
    assert!(
        r.success,
        "a bad {{{{ns:}}}} token outside markdown must not fail install: {} {}",
        r.stdout, r.stderr
    );
    let script = std::fs::read_to_string(sb.mind_home.join("store/skill/review/run.sh")).unwrap();
    assert!(
        script.contains("{{ns:ghost}}"),
        "the token must be left exactly as written outside markdown: {script}"
    );

    // The same bad token, written in the skill's SKILL.md (markdown) instead,
    // still fails install.
    let sb2 = Sandbox::new();
    sb2.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: review\n---\nhand off to {{ns:ghost}}\n",
    );
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2]).success);
    let r2 = sb2.mind(&["learn", "review"]);
    assert!(
        !r2.success,
        "the same bad token in a markdown file must still fail install"
    );
}

#[test]
fn tokens_expand_in_every_markdown_extension_and_no_other() {
    // spec: NS-53 TOOL-19 NS-11
    // The positive half of the markdown gate at install, which the negative
    // tests above cannot prove: a file the predicate ACCEPTS has to actually be
    // scanned. `is_markdown` is a case-insensitive match over four extensions,
    // so a `.markdown`, `.mkd`, `.mdown` or uppercase `.MD` reference doc
    // bundled in a skill gets its tokens expanded exactly like `SKILL.md`,
    // while a near-miss `.mdx`, a `.txt`, an extensionless name and a dotfile
    // whose leading dot is not an extension are all left verbatim. Written as
    // one file per extension so a shrunk extension list fails loudly rather
    // than being absorbed by the "unchanged file" shape of the skip path.
    let sb = Sandbox::new();
    let token = "see {{ns:dev}} and {{self}}\n";
    for name in [
        "A.markdown",
        "B.mkd",
        "C.mdown",
        "D.MD",
        "E.Markdown",
        "F.mdx",
        "G.txt",
        "NOTES",
        ".mkd",
    ] {
        sb.write_and_commit(&format!("skills/review/{name}"), token);
    }

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review", "--yes"]).success);

    let store = sb.mind_home.join("store/skill/jk:review");
    for name in ["A.markdown", "B.mkd", "C.mdown", "D.MD", "E.Markdown"] {
        let got = std::fs::read_to_string(store.join(name)).unwrap();
        assert!(
            !got.contains("{{"),
            "{name} is markdown: every token must expand: {got}"
        );
        assert!(got.contains("see dev and "), "{name}: {got}");
    }
    for name in ["F.mdx", "G.txt", "NOTES", ".mkd"] {
        assert_eq!(
            std::fs::read_to_string(store.join(name)).unwrap(),
            token,
            "{name} is not markdown: its tokens must be left exactly as written"
        );
    }
}

#[test]
fn meld_as_warns_about_unguarded_prose_references() {
    // spec: NS-20, NS-22, NS-42, CLI-14, CLI-162
    // Without --verbose the unguarded-reference warning is suppressed (CLI-162).
    // With --verbose it is emitted for non-agent referents whose name would be
    // prefixed; NS-42: agent-only names are excluded from the warning.
    // This fixture references `review` (a skill sibling) in bare prose.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the review skill.\n",
    );
    let spec = sb.source_spec();

    // Default (no --verbose): warning is suppressed.
    let r = sb.mind(&["meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "expected no unguarded-ref warning without --verbose: {}",
        r.stderr
    );

    // With --verbose: warning is emitted.
    let sb2 = Sandbox::new();
    sb2.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the review skill.\n",
    );
    let spec2 = sb2.source_spec();
    let r2 = sb2.mind(&["--verbose", "meld", &spec2, "--as", "jk"]);
    assert!(r2.success, "{}", r2.stderr);
    assert!(
        r2.stderr.contains("references sibling(s) in prose") && r2.stderr.contains("review"),
        "expected unguarded-ref warning under --verbose: {}",
        r2.stderr
    );
}

#[test]
fn meld_as_does_not_warn_for_a_sibling_mention_inside_a_code_span() {
    // spec: NS-20 NS-55
    // The unguarded-reference warning is structure-aware (NS-55) on every
    // surface that calls it, not just `review`: a sibling name that sits
    // inside a code span is not a real reference, so `meld --verbose` must not
    // flag it either, even though a whole-word text scan (the pre-NS-55
    // behavior) would have.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nRun `review` locally first.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["--verbose", "meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "a code-span mention is not a reference and must not be flagged: {}",
        r.stderr
    );
}

#[test]
fn no_warning_when_unprefixed() {
    // spec: NS-23, CLI-162 -- no prefix in effect => no warning, EVEN under
    // --verbose. The meld runs with --verbose so the warning gate is open; the
    // only reason it stays silent is the absent prefix (NS-23). A fixture that
    // WOULD warn under a prefix (bare prose ref to the skill sibling `review`)
    // must still stay silent unprefixed.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent, then run the review skill.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["--verbose", "meld", &spec]); // no prefix -> bare refs are correct
    assert!(r.success);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "{}",
        r.stderr
    );
}

// ---- NS-40 / NS-41 / NS-42: agents not namespaced -------------------------

#[test]
fn prefixed_agent_links_under_bare_harness_name() {
    // spec: NS-40 -- an agent from a prefixed source installs its store copy
    // under the effective (prefixed) name but creates the agent-home link under
    // the bare frontmatter `name`, which is how the harness resolves the agent.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    // Store uses the effective prefixed name.
    assert!(
        sb.mind_home.join("store/agent/jk:dev").exists(),
        "store should be at jk:dev"
    );
    // Agent-home link is under the bare frontmatter name, not the prefixed one.
    assert!(
        sb.claude_home.join("agents/dev.md").exists(),
        "link should be agents/dev.md"
    );
    assert!(
        !sb.claude_home.join("agents/jk:dev.md").exists(),
        "no link should exist at the prefixed path"
    );
}

#[test]
fn prefixed_agent_manifest_key_uses_effective_name() {
    // spec: NS-40 -- the manifest key and `recall` output use the effective
    // (prefixed) name; only the link is bare.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("jk:dev"),
        "recall should show jk:dev: {}",
        r.stdout
    );
}

#[test]
fn agent_collision_is_refused_at_learn() {
    // spec: NS-41 -- installing an agent whose bare harness name conflicts with
    // an installed agent from a different source is refused with AgentCollision.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // A second source ships an agent with the same frontmatter name "dev".
    let other = Sandbox::bare("other");
    other.write_and_commit(
        "agents/coder.md",
        "---\nname: dev\ndescription: another dev\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &other.source_spec()]).success);

    let r = sb.mind(&["learn", "coder"]);
    assert!(!r.success, "colliding agent install must be refused");
    assert!(
        r.stderr.contains("conflict"),
        "expected collision error message: {}",
        r.stderr
    );
}

#[test]
fn meld_warns_when_incoming_agent_would_collide() {
    // spec: NS-41 -- `meld` emits an advisory warning (not an error) when the
    // incoming source carries an agent that would collide with an installed one.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    let other = Sandbox::bare("other");
    other.write_and_commit(
        "agents/coder.md",
        "---\nname: dev\ndescription: another dev\n---\n# dev\n",
    );
    // Meld must succeed (advisory only).
    let r = sb.mind(&["meld", &other.source_spec()]);
    assert!(
        r.success,
        "meld should succeed even on collision: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("would collide"),
        "expected advisory collision warning: {}",
        r.stderr
    );
}

#[test]
fn no_agent_collision_when_reinstalling_same_source() {
    // spec: NS-41 -- re-learning the same agent (same source + bare name) is not
    // a collision; upgrade / re-install must succeed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);
    // Re-learn the same item: should succeed, not collide.
    let r = sb.mind(&["learn", "dev"]);
    assert!(
        r.success,
        "re-learn of same agent should succeed: {}",
        r.stderr
    );
}

#[test]
fn agent_token_expands_bare_under_prefix() {
    // spec: NS-42 -- a {{ns:name}} token whose referent is a sibling agent
    // expands to the bare name even when the source has a prefix.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:dev}} for coding.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);

    // The stored content uses the bare name for the agent referent.
    let store = sb.mind_home.join("store/agent/jk:lead");
    let body = std::fs::read_to_string(&store).expect("store file");
    assert!(
        body.contains("dev") && !body.contains("jk:dev"),
        "agent token should expand bare: {body}"
    );
}

#[test]
fn unguarded_ref_warning_skips_agent_only_sibling_names() {
    // spec: NS-42, CLI-162 -- bare prose references to a sibling agent do not
    // trigger the unguarded-reference warning: the agent links bare regardless of
    // prefix, so the prose reference resolves correctly even without a token. The
    // meld runs under --verbose (CLI-162), so the warning IS active for non-agent
    // siblings; the fixture references BOTH the skill sibling `review` (which MUST
    // be flagged) and the agent sibling `dev` (which MUST NOT be) so the assertion
    // distinguishes "excluded because agent" from "warning never fired at all".
    let sb = Sandbox::new();
    // The standard fixture has `agents/dev.md` (agent) and `skills/review`.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent, then run the review skill.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["--verbose", "meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    // The warning fires for the non-agent sibling `review`...
    assert!(
        r.stderr.contains("references sibling(s) in prose") && r.stderr.contains("review"),
        "non-agent sibling `review` must be flagged under --verbose: {}",
        r.stderr
    );
    // ...but `dev` is an agent sibling and must NOT appear in the warning.
    assert!(
        !r.stderr.contains("dev"),
        "agent-only sibling should not trigger unguarded-ref warning: {}",
        r.stderr
    );
}

#[test]
fn unprefixed_agent_links_under_frontmatter_name_not_filename() {
    // spec: NS-40 -- even with no prefix, an agent links under its frontmatter
    // `name`, which may differ from its filename. The store copy and stable
    // identity use the file stem; only the agent-home link uses the harness name.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/coder.md",
        "---\nname: reviewer\ndescription: reviews code\n---\n# reviewer\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "coder"]).success);
    // The link uses the frontmatter name, not the filename.
    assert!(
        sb.claude_home.join("agents/reviewer.md").exists(),
        "link should use the frontmatter name"
    );
    assert!(
        !sb.claude_home.join("agents/coder.md").exists(),
        "no link at the filename path"
    );
    // Store and identity use the file stem.
    assert!(sb.mind_home.join("store/agent/coder").exists());
}

#[test]
fn upgrade_moves_agent_link_when_frontmatter_name_changes() {
    // spec: NS-40 -- an agent links under its frontmatter `name`. When the source
    // changes that name but keeps the same filename (so the item's effective name
    // and stable identity are unchanged, i.e. this is an in-place content upgrade,
    // not a rename), `upgrade` must move the agent-home link to the new bare name
    // and leave no orphaned old link.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.claude_home.join("agents/dev.md").exists());

    // Same file, new frontmatter name.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: lead\ndescription: now the lead\n---\n# lead agent\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    // The link moved to the new harness name; the old one is not orphaned.
    assert!(
        sb.claude_home.join("agents/lead.md").exists(),
        "link should move to the new harness name"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err(),
        "the old harness-name link must not be left orphaned"
    );
    // The store path and identity are unchanged (keyed on the file stem).
    assert!(sb.mind_home.join("store/agent/dev").exists());
}

#[test]
fn introspect_fix_recreates_bare_agent_link() {
    // spec: NS-40 -- `introspect --fix` recreates a missing agent link at its
    // recorded (bare) path from the manifest link registry, not a prefixed path.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    let link = sb.claude_home.join("agents/dev.md");
    assert!(link.exists());
    // Simulate drift: the link is deleted out from under mind.
    std::fs::remove_file(&link).unwrap();
    assert!(sb.mind(&["introspect", "--fix"]).success);
    assert!(
        link.exists(),
        "introspect --fix must recreate the bare link"
    );
    assert!(
        !sb.claude_home.join("agents/jk:dev.md").exists(),
        "the recreated link must be bare, not prefixed"
    );
}

#[test]
fn forget_removes_bare_agent_link() {
    // spec: NS-40 -- `forget` removes the agent link at its recorded bare path via
    // the manifest link registry, even when the source is prefixed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    assert!(sb.claude_home.join("agents/dev.md").exists());
    assert!(sb.mind(&["forget", "jk:dev"]).success);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err(),
        "forget must remove the bare agent link"
    );
    assert!(!sb.mind_home.join("store/agent/jk:dev").exists());
}

#[test]
fn cross_kind_shadow_name_still_warns_in_prose_under_prefix() {
    // spec: NS-42 -- the cross-kind shadow rule: a name that is BOTH an agent and
    // a skill is NOT treated as a bare agent referent, so a bare prose reference
    // to it under a prefix still triggers the unguarded-reference warning (the
    // skill side would be prefixed and break).
    let sb = Sandbox::new();
    // `shared` exists as both a skill and an agent.
    sb.write_and_commit(
        "skills/shared/SKILL.md",
        "---\nname: shared\ndescription: shared skill\n---\n# shared\n",
    );
    sb.write_and_commit(
        "agents/shared.md",
        "---\nname: shared\ndescription: shared agent\n---\n# shared\n",
    );
    // Another item references `shared` in bare prose.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nHand off to shared for the work.\n",
    );
    let r = sb.mind(&["--verbose", "meld", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("shared"),
        "a name shadowed across agent+skill must still be flagged: {}",
        r.stderr
    );
}

// ---- end NS-40 / NS-41 / NS-42 -------------------------------------------

#[test]
fn unguarded_ref_warning_scans_all_files_of_an_item() {
    // spec: NS-20, NS-42
    let sb = Sandbox::new();
    // A skill whose bare prose reference to the skill sibling `review` lives in
    // a secondary file, not SKILL.md. The warning must still catch it (scan is
    // item-wide). Using a skill referent (not an agent) because NS-42 excludes
    // pure-agent sibling names from the warning scan.
    sb.write_and_commit(
        "skills/lead/SKILL.md",
        "---\nname: lead\ndescription: lead skill\n---\n# lead\n",
    );
    sb.write_and_commit("skills/lead/NOTES.md", "Run the review skill first.\n");

    let r = sb.mind(&["--verbose", "meld", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("skill:jk:lead") && r.stderr.contains("review"),
        "warning should cite a sibling ref found in a non-SKILL.md file: {}",
        r.stderr
    );
}

#[test]
fn example_namespacing_expands_references() {
    // spec: NS-11, NS-14
    // Prefixed: tokens expand to the prefixed effective names, and a guarded
    // source produces no unguarded-reference warning.
    let jk = Sandbox::from_example("namespacing");
    // --verbose (CLI-162) opens the warning gate so the "no warning" assertion is
    // genuine: the source is prefixed, yet all refs are tokens, so nothing fires.
    let meld = jk.mind(&["--verbose", "meld", &jk.source_spec(), "--as", "jk"]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(
        !meld.stderr.contains("references sibling(s) in prose"),
        "all refs are tokens, so no warning: {}",
        meld.stderr
    );
    // `lead` references siblings via {{ns:}}, so a partial learn pulls in the
    // closure and prompts (DEP-31); `--yes` confirms.
    assert!(jk.mind(&["learn", "jk:lead", "--yes"]).success);
    let lead = std::fs::read_to_string(jk.mind_home.join("store/agent/jk:lead")).unwrap();
    // NS-42: {{ns:dev}} is an agent sibling -- expands bare under prefix too.
    assert!(lead.contains("the dev agent"), "{lead}");
    assert!(
        !lead.contains("jk:dev"),
        "agent token must not be prefixed: {lead}"
    );
    // Skill and rule referents still expand with the prefix (NS-11).
    assert!(lead.contains("the jk:review skill"), "{lead}");
    assert!(lead.contains("the jk:style rule"), "{lead}");
    assert!(!lead.contains("{{ns:"), "tokens should be gone: {lead}");
    // The skill references a rule from inside its directory; it expands too.
    assert!(jk.mind(&["learn", "jk:review", "--yes"]).success);
    let review =
        std::fs::read_to_string(jk.mind_home.join("store/skill/jk:review/SKILL.md")).unwrap();
    assert!(review.contains("jk:style rule"), "{review}");
    assert!(!review.contains("{{ns:"), "tokens should be gone: {review}");

    // Unprefixed: the same tokens expand to the bare names.
    let bare = Sandbox::from_example("namespacing");
    assert!(bare.mind(&["meld", &bare.source_spec()]).success);
    assert!(bare.mind(&["learn", "lead", "--yes"]).success);
    let lead2 = std::fs::read_to_string(bare.mind_home.join("store/agent/lead")).unwrap();
    assert!(lead2.contains("the dev agent"), "{lead2}");
    assert!(lead2.contains("the review skill"), "{lead2}");
    assert!(lead2.contains("the style rule"), "{lead2}");
    assert!(!lead2.contains("{{ns:"), "{lead2}");
}

#[test]
fn two_sources_same_names_coexist_under_a_prefix() {
    // spec: NS-2
    // Two melded sources both ship `review`/`dev`/`style`. Namespacing the second
    // gives its items distinct effective names, so both install side by side.
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec(), "--as", "zz"]).success);

    // The prefix makes the effective names distinct, so each installs by its own
    // name with no ambiguity and no qualifier: `review` from a, `zz:review` from b.
    let la = a.mind(&["learn", "review"]);
    assert!(la.success, "learn review: {} {}", la.stdout, la.stderr);
    let lb = a.mind(&["learn", "zz:review"]);
    assert!(lb.success, "learn zz:review: {} {}", lb.stdout, lb.stderr);

    // Both coexist: the unprefixed one as `review`, the namespaced one as `zz:review`.
    let recall = a.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("skill:zz:review"), "{recall}");
    assert!(
        a.mind_home.join("store/skill/review").is_dir(),
        "a's store copy"
    );
    assert!(
        a.mind_home.join("store/skill/zz:review").is_dir(),
        "b's store copy"
    );
    for link in ["skills/review", "skills/zz:review"] {
        assert!(
            std::fs::symlink_metadata(a.claude_home.join(link))
                .unwrap()
                .file_type()
                .is_symlink(),
            "expected a symlink at {link}"
        );
    }
}

#[test]
fn unprefixed_same_name_second_install_is_a_noop_first_wins() {
    // spec: NS-2
    // Without a prefix two same-named items share one install path (`skill:review`),
    // so they cannot coexist. The first installed wins; a later install of the same
    // name from the other source is a no-op (the name is already taken), not a
    // silent overwrite -- and it is not an error.
    let a = Sandbox::new();
    let b = Sandbox::new();
    // Give b's review a distinct description so an overwrite would be observable.
    b.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: BRAVO review\n---\n# review b\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());
    assert!(a.mind(&["learn", &format!("{a_full}#review")]).success);
    // Installing the same name from the other source succeeds but changes nothing.
    let second = a.mind(&["learn", &format!("{b_full}#review")]);
    assert!(second.success, "second install: {}", second.stderr);

    // The store still holds a's content: the first install was not replaced.
    let installed =
        std::fs::read_to_string(a.mind_home.join("store/skill/review/SKILL.md")).unwrap();
    assert!(
        installed.contains("Review the diff for bugs") && !installed.contains("BRAVO review"),
        "the first install must remain (no overwrite): {installed}"
    );
}

// ---------------------------------------------------------------------------
// Output polish: capability gate (CLI-151/154), glyph fallback (CLI-152), and
// the structured JSON result object for mutating verbs (CLI-153).
//
// The integration harness always pipes stdout (non-TTY), so the color/Unicode
// capability gate (CLI-151) is OFF: output must be plain ASCII with no ANSI
// escape sequences. The rich (TTY) branch of the gate cannot be exercised
// without a real PTY and is covered by unit tests in src/render.rs.
// ---------------------------------------------------------------------------

#[test]
fn meld_as_reserved_kind_word_is_rejected() {
    // spec: NS-25
    // A prefix equal to a reserved item-kind word (skill/agent/rule/tool) would
    // make `prefix:name` indistinguishable from a kind-qualified item ref (NS-26),
    // so `meld --as <kind-word>` must fail before registering the source.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // `--as skill` must be rejected.
    let r = sb.mind(&["meld", &spec, "--as", "skill"]);
    assert!(
        !r.success,
        "meld --as skill must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("reserved") || r.stderr.contains("kind"),
        "error must mention the reserved-kind-word problem: {}",
        r.stderr
    );
    // Source must not have been registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "source must not be registered after a rejected meld"
    );

    // `--as agent` must also be rejected (second reserved word).
    let sb2 = Sandbox::new();
    let spec2 = sb2.source_spec();
    let r2 = sb2.mind(&["meld", &spec2, "--as", "agent"]);
    assert!(
        !r2.success,
        "meld --as agent must fail: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stderr.contains("reserved") || r2.stderr.contains("kind"),
        "error must mention the reserved-kind-word problem: {}",
        r2.stderr
    );
}

// ---------------------------------------------------------------------------
// NS-27: item installed under former `-` separator migrates on upgrade
// ---------------------------------------------------------------------------

#[test]
fn upgrade_migrates_dash_separator_to_colon_by_stable_identity() {
    // spec: NS-27
    // An item installed under the former `-` separator (e.g. `jk-review`) keeps
    // its stable identity (source, kind, bare_name). When the binary now emits `:`
    // (so the catalog yields `jk:review`), upgrade must detect the rename via
    // identity match and re-link under the new separator, removing the old path.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Meld with prefix `jk`; the binary now emits `jk:review`.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(
        sb.mind(&["learn", "jk:review"]).success,
        "learn must accept the colon-separated effective name"
    );

    // Verify the current (colon-separator) layout.
    let new_store = sb.mind_home.join("store/skill/jk:review");
    let new_link = sb.claude_home.join("skills/jk:review");
    assert!(
        new_store.exists(),
        "store must be at jk:review after install"
    );
    assert!(
        std::fs::symlink_metadata(&new_link).is_ok(),
        "symlink must be at skills/jk:review after install"
    );

    // -- Simulate old `-` separator layout --
    // Rewrite the manifest so it looks like an install from a binary that
    // used `-` as the separator (e.g. jk-review instead of jk:review).
    let manifest_path = sb.mind_home.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap();
    // Replace the effective name in the JSON key, the name field, the store
    // path, and the absolute link paths, while keeping bare_name/source/kind
    // intact (those form the stable identity).
    let manifest_old = manifest_text
        .replace("\"skill:jk:review\"", "\"skill:jk-review\"")
        .replace("\"jk:review\"", "\"jk-review\"")
        .replace("store/skill/jk:review", "store/skill/jk-review")
        .replace("skills/jk:review", "skills/jk-review");
    std::fs::write(&manifest_path, &manifest_old).unwrap();

    // Move the store directory to the old name.
    let old_store = sb.mind_home.join("store/skill/jk-review");
    std::fs::rename(&new_store, &old_store).unwrap();

    // Recreate the symlink under the old name pointing at the renamed store.
    std::fs::remove_file(&new_link).unwrap();
    let old_link = sb.claude_home.join("skills/jk-review");
    std::os::unix::fs::symlink(&old_store, &old_link).unwrap();

    // -- Run upgrade --
    // The catalog finds (source, kind=skill, bare_name=review, effective_name=jk:review).
    // The manifest has (source, kind=skill, bare_name=review, name=jk-review).
    // Identity match fires: new effective name != recorded name => rename.
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        up.success,
        "upgrade must succeed on a separator-migration rename: {} {}",
        up.stdout, up.stderr
    );
    // The upgrade output must mention both the old and new names in the rename report.
    assert!(
        up.stdout.contains("jk-review") && up.stdout.contains("jk:review"),
        "upgrade must report the rename jk-review -> jk:review: {}",
        up.stdout
    );

    // After upgrade: new-separator paths must exist.
    assert!(
        sb.mind_home.join("store/skill/jk:review").exists(),
        "store must be at jk:review after separator migration"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_ok(),
        "symlink must be at skills/jk:review after separator migration"
    );
    // Old-separator paths must be gone.
    assert!(
        !sb.mind_home.join("store/skill/jk-review").exists(),
        "old jk-review store must be removed after migration"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk-review")).is_err(),
        "old jk-review symlink must be removed after migration"
    );
    // Recall must now show the new name.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:jk:review"),
        "recall must show skill:jk:review after migration: {recall}"
    );
    assert!(
        !recall.contains("skill:jk-review"),
        "recall must not show the old skill:jk-review after migration: {recall}"
    );
}

// ---------------------------------------------------------------------------
// NS-26: a prefixed effective name (`jk:review`) is usable as a ref in the
// installed-side verbs (recall / upgrade / forget), not only `learn`.
// ---------------------------------------------------------------------------

#[test]
fn prefixed_effective_name_resolves_for_recall_upgrade_and_forget() {
    // spec: NS-26
    // NS-26 promises prefixed effective names stay usable as refs despite the
    // shared `:` separator. `learn jk:review` is covered elsewhere; this exercises
    // the manifest-side resolution path (resolve_installed) used by recall,
    // upgrade, and forget, which the resolve.rs unit test does not drive end to end.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(
        sb.mind(&["learn", "jk:review"]).success,
        "learn must accept the colon-separated effective name"
    );

    // recall <item> resolves the install by its effective name.
    let recall = sb.mind(&["recall", "jk:review"]);
    assert!(
        recall.success && recall.stdout.contains("jk:review"),
        "recall jk:review must resolve the installed item: {} {}",
        recall.stdout,
        recall.stderr
    );

    // upgrade <item> resolves it too (here a no-op, but it must not be reported
    // as NotInstalled, which is what a broken `:`-name parse would produce).
    let upgrade = sb.mind(&["upgrade", "jk:review", "--yes"]);
    assert!(
        upgrade.success,
        "upgrade jk:review must resolve the installed item: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stderr.contains("not installed") && !upgrade.stdout.contains("not installed"),
        "upgrade jk:review must not report the prefixed ref as not installed: {} {}",
        upgrade.stdout,
        upgrade.stderr
    );

    // forget <item> removes exactly that item, by its effective name.
    let forget = sb.mind(&["forget", "jk:review"]);
    assert!(
        forget.success,
        "forget jk:review must resolve and remove the installed item: {} {}",
        forget.stdout, forget.stderr
    );
    assert!(
        !sb.mind_home.join("store/skill/jk:review").exists(),
        "forget jk:review must remove the store copy"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_err(),
        "forget jk:review must remove the lobe symlink"
    );
    // It is no longer in the manifest: forgetting the same effective name again
    // must now report it as not installed (proving the manifest entry was keyed
    // and removed by the `:` effective name, not left behind).
    let again = sb.mind(&["forget", "jk:review"]);
    assert!(
        !again.success
            && (again.stderr.contains("not installed") || again.stderr.contains("jk:review")),
        "a second forget jk:review must report it as not installed: {} {}",
        again.stdout,
        again.stderr
    );
}

// ---------------------------------------------------------------------------
// NS-2: a `:` in the effective name produces a real, resolvable on-disk store
// dir and lobe symlink on this platform (the load-bearing assumption).
// ---------------------------------------------------------------------------

#[test]
fn prefixed_lobe_symlink_resolves_through_colon_path() {
    // spec: NS-2
    // The whole separator change rests on a literal `:` being valid in a store
    // directory and a lobe symlink. Existing tests assert the paths EXIST; this
    // asserts they are RESOLVABLE: the file is readable THROUGH the colon-bearing
    // symlink and its content matches the store copy. If a `:` in a path broke on
    // this platform, this would fail rather than silently certify a broken layout.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);

    let store_file = sb.mind_home.join("store/skill/jk:review/SKILL.md");
    let link_file = sb.claude_home.join("skills/jk:review/SKILL.md");

    // The lobe entry is a symlink (not a copy).
    let link_dir = sb.claude_home.join("skills/jk:review");
    assert!(
        std::fs::symlink_metadata(&link_dir)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the prefixed lobe entry must be a symlink"
    );

    // Reading THROUGH the colon-bearing symlink path must resolve to the store
    // copy: the file is readable and its bytes match.
    let via_store = std::fs::read_to_string(&store_file).expect("store file readable");
    let via_link =
        std::fs::read_to_string(&link_file).expect("file readable through the colon-bearing link");
    assert_eq!(
        via_link, via_store,
        "content read through the `:` symlink must equal the store copy"
    );
    assert!(
        via_link.contains("review skill"),
        "the resolved file must be the review SKILL.md: {via_link}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-5 / NS-2: dump of a prefixed install records the prefix as `as = "<p>"`
// and lists items by their BARE `kind:name`, not the prefixed effective name.
// ---------------------------------------------------------------------------

#[test]
fn remeld_as_reserved_kind_word_is_rejected() {
    // spec: NS-25
    // A source already melded can have its prefix changed by re-melding with
    // `--as` (CLI-13). That change must be held to the same NS-25 rule, so a
    // reserved kind word is rejected at the re-meld chokepoint too. Also covers
    // `rule` and `tool`, which the initial-meld CLI test does not exercise.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // First meld with a normal prefix so the source is registered.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);

    for word in ["rule", "tool"] {
        let r = sb.mind(&["meld", &spec, "--as", word]);
        assert!(
            !r.success,
            "re-meld --as {word} must fail: stdout={} stderr={}",
            r.stdout, r.stderr
        );
        assert!(
            r.stderr.contains("reserved") || r.stderr.contains("kind"),
            "error must mention the reserved-kind-word problem for {word}: {}",
            r.stderr
        );
    }

    // The original prefix must be intact (the rejected re-meld changed nothing).
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "the original prefix must survive a rejected re-meld: {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Claude plugin marketplace support (MKT-1..11)
// ---------------------------------------------------------------------------

#[test]
fn marketplace_two_plugins_same_agent_name_collide() {
    // spec: NS-41
    // Per MKT-5/NS-40 a plugin's agents flatten to their bare frontmatter name and
    // the plugin prefix does NOT reach agents. So two catalog plugins each shipping
    // an agent with the SAME frontmatter `name:` both target agents/<name>.md -- a
    // detected collision (NS-41), not a silent overwrite. Learning the second one
    // must fail with an agent-collision error.
    let catalog = Sandbox::bare("mkt-agent-collide");
    // Plugin A ships an agent whose frontmatter name is "shared".
    catalog.write_and_commit("plugins/a/.claude-plugin/plugin.json", r#"{"name":"pa"}"#);
    catalog.write_and_commit(
        "plugins/a/agents/from-a.md",
        "---\nname: shared\ndescription: agent from A\n---\n# a\n",
    );
    // Plugin B ships a differently-filed agent with the SAME frontmatter name.
    catalog.write_and_commit("plugins/b/.claude-plugin/plugin.json", r#"{"name":"pb"}"#);
    catalog.write_and_commit(
        "plugins/b/agents/from-b.md",
        "---\nname: shared\ndescription: agent from B\n---\n# b\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"pa","source":"./plugins/a"},{"name":"pb","source":"./plugins/b"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // Both agents flatten to the bare harness name "shared" (prefix omitted).
    // Learn the first: it installs at agents/shared.md.
    let r1 = catalog.mind(&["learn", "pa:from-a"]);
    assert!(
        r1.success,
        "first agent must install: {} {}",
        r1.stdout, r1.stderr
    );
    assert!(
        catalog.claude_home.join("agents/shared.md").exists(),
        "first agent must link at agents/shared.md"
    );

    // Learn the second (a different source): NS-41 must refuse the colliding link.
    let r2 = catalog.mind(&["learn", "pb:from-b"]);
    assert!(
        !r2.success,
        "a second agent colliding at agents/shared.md must be refused (NS-41): {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stderr.to_lowercase().contains("collid") || r2.stderr.to_lowercase().contains("shared"),
        "the error must describe the agent collision: {}",
        r2.stderr
    );
}

#[test]
fn skill_collision_non_interactive_errors_with_guidance() {
    // spec: NS-43 NS-45
    // In a non-interactive session (no TTY), melding a source whose skill
    // effective name matches an already-installed skill from a different source
    // exits non-zero (SkillCollision) and suggests --namespace. Melding with
    // --namespace resolves the conflict.
    let src_a = Sandbox::bare("ns43-a");
    src_a.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill A\n---\n# deploy\n",
    );
    // Meld and install source A so `deploy` is in the manifest.
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    // Source B has the same skill name.
    let src_b = Sandbox::bare("ns43-b");
    src_b.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill B\n---\n# deploy\n",
    );
    // Non-TTY meld of source B must fail with SkillCollision (NS-45).
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "colliding meld must fail in non-interactive mode (NS-45)"
    );
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("collision")
            || combined.contains("conflict")
            || combined.contains("namespace"),
        "error must describe the collision and suggest --namespace (NS-43 NS-45): {combined}"
    );

    // Melding source B with --namespace resolves the collision.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--namespace", "sb", "--yes"]);
    assert!(
        r.success,
        "meld with --namespace must resolve the collision: {} {}",
        r.stdout, r.stderr
    );
    // The namespaced skill is discoverable in probe.
    let probe = src_a.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("sb:deploy"),
        "namespaced skill sb:deploy must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn skill_collision_same_source_remeld_is_not_a_collision() {
    // spec: NS-43
    // The NS-43 same-source skip prevents false collision detection when items
    // from a previously installed source remain in the manifest after the source
    // is unregistered. Re-melding that source must succeed.
    let src = Sandbox::bare("ns43-remeld");
    src.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill\n---\n# deploy\n",
    );

    // Meld and install.
    let r = src.mind(&["meld", &src.source_spec(), "--yes"]);
    assert!(
        r.success,
        "first meld must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Unmeld --unlink-only: source is removed from the registry but the
    // installed item remains in the manifest under the original source name.
    let r = src.mind(&["unmeld", "ns43-remeld", "--unlink-only", "--yes"]);
    assert!(
        r.success,
        "unmeld --unlink-only must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Re-meld: manifest still holds `deploy` under this source's name.
    // The NS-43 same-source skip must prevent a false SkillCollision.
    let r = src.mind(&["meld", &src.source_spec(), "--yes"]);
    assert!(
        r.success,
        "re-meld of same source must succeed (NS-43 same-source skip): {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn skill_collision_with_namespace_set_does_not_collide() {
    // spec: NS-43
    // Melding with --namespace makes effective names distinct from
    // already-installed items, so the NS-43 collision check does not fire.
    let src_a = Sandbox::bare("ns43-ns-a");
    src_a.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill A\n---\n# deploy\n",
    );
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    let src_b = Sandbox::bare("ns43-ns-b");
    src_b.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill B\n---\n# deploy\n",
    );
    // --namespace sb makes the effective name "sb:deploy", distinct from the
    // installed "deploy" -- no collision fires.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--namespace", "sb", "--yes"]);
    assert!(
        r.success,
        "meld with --namespace must succeed when effective names are distinct (NS-43): {} {}",
        r.stdout, r.stderr
    );
    // Both skills are discoverable under their distinct names.
    let probe = src_a.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("sb:deploy"),
        "namespaced sb:deploy must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn skill_collision_yes_flag_forces_noninteractive_error() {
    // spec: NS-43 NS-45
    // Passing --yes to `meld` that would produce a cross-source skill collision
    // must cause SkillCollision (non-zero exit) rather than hanging on an
    // interactive prompt, even in a session where is_tty() might be true.
    // In the headless test environment is_tty()=false, so this test also
    // functions as a regression guard that the non-interactive path fires.
    let src_a = Sandbox::bare("h1-yes-a");
    src_a.write_and_commit(
        "skills/h1skill/SKILL.md",
        "---\nname: h1skill\ndescription: H1 skill A\n---\n# h1skill\n",
    );
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    let src_b = Sandbox::bare("h1-yes-b");
    src_b.write_and_commit(
        "skills/h1skill/SKILL.md",
        "---\nname: h1skill\ndescription: H1 skill B\n---\n# h1skill\n",
    );
    // --yes + collision: must return non-zero (SkillCollision) rather than prompt.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "--yes meld with collision must fail non-interactively (NS-45): {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("collision")
            || combined.contains("conflict")
            || combined.contains("namespace"),
        "--yes collision error must mention collision/namespace (NS-43 NS-45): {combined}"
    );
}

// ---------------------------------------------------------------------------
// M1: default `recall` view emits `namespace:` not `as:` for the alias token
// ---------------------------------------------------------------------------

#[test]
fn recall_default_view_uses_namespace_prefix_for_alias() {
    // spec: NS-43 NS-45
    // The non-`--sources` recall path (the default item status view) must emit
    // `namespace:<alias>` in the source header, consistent with the `--sources`
    // path (which already uses `namespace:`).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--namespace", "jk"]).success);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("namespace:jk"),
        "default recall must show 'namespace:jk' not 'as:jk': {recall}"
    );
    assert!(
        !recall.contains("as:jk"),
        "default recall must not use the old 'as:' prefix: {recall}"
    );
}

// ---------------------------------------------------------------------------
// M14 / DSC-78: sync re-walk honors `namespace =` key in [discover].sources
// ---------------------------------------------------------------------------
