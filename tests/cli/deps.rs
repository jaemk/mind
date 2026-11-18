//! Item dependencies (spec/dependencies.md).

use super::*;

/// A source whose skill `review` references the agent `reviewer` via a
/// `{{ns:}}` token, so a partial `learn skill:review` must pull in `reviewer`
/// (its intra-source dependency). Returns the melded sandbox.
fn dep_fixture() -> Sandbox {
    let sb = Sandbox::bare("agents-and-skills");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\nhand off to {{ns:reviewer}}\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviews changes\n---\n# reviewer agent\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn learn_yes_installs_referenced_dependency_closure() {
    // spec: DEP-30
    // A partial `learn skill:review --yes` installs the whole closure: the
    // selected skill AND the agent it references via {{ns:reviewer}}. Both are
    // recorded in the manifest (dependency-first install order is internal and
    // not directly observable, so we assert the closure was applied).
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "referenced dependency pulled into the closure: {recall}"
    );
}

#[test]
fn learn_whole_source_glob_pulls_no_extras() {
    // spec: DEP-10 DEP-31
    // Selecting the whole source is full coverage: resolution is a no-op, so
    // `learn` installs directly with no prompt and adds nothing beyond the
    // two items that are already the entire source.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "agents-and-skills#*"]);
    assert!(r.success, "{}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:reviewer"), "{recall}");
}

#[test]
fn learn_dependency_dry_run_renders_tree_and_installs_nothing() {
    // spec: DEP-32
    // `--dry-run` over a partial selection renders the dependency tree (which
    // names the pulled-in `reviewer`) and lists the closure, but installs
    // nothing: the manifest stays empty.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--dry-run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("would learn"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skill:review [selected]"),
        "tree should head with the selected skill: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should mark the pulled-in dependency: {}",
        r.stdout
    );

    // Nothing was installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());
}

#[test]
fn forget_does_not_remove_a_dependency() {
    // spec: DEP-50
    // After installing the closure, forgetting the skill leaves its pulled-in
    // dependency installed: `forget` is per-item and never auto-removes deps.
    let sb = dep_fixture();
    assert!(sb.mind(&["learn", "skill:review", "--yes"]).success);
    assert!(sb.mind(&["forget", "skill:review"]).success);

    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "the forgotten skill is gone"
    );
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "the dependency stays installed"
    );
}

#[test]
fn learn_installs_dependency_before_dependent() {
    // spec: DEP-30 DEP-21
    // The closure installs dependency-first: the agent `reviewer` (a pulled-in
    // dependency) installs BEFORE the skill `review` that references it. The
    // "learned ..." lines are emitted in install order, so the dependency line
    // must precede the dependent's line in stdout.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    let dep_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned agent:reviewer "))
        .unwrap_or_else(|| panic!("missing reviewer learned line: {}", r.stdout));
    let dependent_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned skill:review "))
        .unwrap_or_else(|| panic!("missing review learned line: {}", r.stdout));
    assert!(
        dep_line < dependent_line,
        "dependency must install before its dependent: {}",
        r.stdout
    );
}

#[test]
fn learn_dependency_prompt_decline_installs_nothing() {
    // spec: DEP-31
    // When the closure adds a pulled-in dependency, `learn` (no --yes) prints
    // the tree and prompts. Answering "n" cancels: nothing is installed, the
    // manifest holds neither item, and no symlinks are created.
    let sb = dep_fixture();
    let r = sb.mind_tty(&["learn", "skill:review"], "n\n");
    assert!(r.success, "{}", r.stderr);
    // The dependency tree is shown before the prompt.
    assert!(
        r.stdout.contains("skill:review [selected]"),
        "tree should head with the selected skill: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should mark the pulled-in dependency: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("cancelled; nothing installed"),
        "decline should print the cancelled line: {}",
        r.stdout
    );

    // Nothing installed: manifest empty, no symlinks for either item.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());
}

#[test]
fn learn_dependency_json_without_yes_refuses() {
    // spec: DEP-31, LIFE-45 -- M1: `--json` cannot show the DEP-31 prompt, so it
    // must refuse a dependency-pulling `learn` without `--yes` (ConfirmationRequired,
    // exit 1) rather than install the whole closure unprompted.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--json"]);
    assert!(
        !r.success,
        "--json learn that pulls in a dependency must refuse without --yes: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("confirmation-required"),
        "the json envelope should carry the confirmation-required kind: {}",
        r.stdout
    );
    // Nothing was installed: neither the selected skill nor its pulled-in dep.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());

    // With --yes the whole closure installs (the refusal is only about consent).
    assert!(
        sb.mind(&["learn", "skill:review", "--json", "--yes"])
            .success
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok());
}

#[test]
fn learn_dependency_prompt_defaults_to_no_on_eof() {
    // spec: DEP-31 CLI-258 CLI-259
    // At a terminal, EOF on the prompt takes the `[y/N]` default, No, so the
    // closure is not installed. The prompt and tree are still shown.
    let sb = dep_fixture();
    let r = sb.mind_tty(&["learn", "skill:review"], "");
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should still render before the prompt: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("cancelled; nothing installed"),
        "EOF should default to No: {}",
        r.stdout
    );

    // Nothing installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());

    // Without a terminal the prompt is never shown: the run refuses and points
    // at --yes, even with a "y" piped in.
    let piped = sb.mind_with_input(&["learn", "skill:review"], Some("y\n"));
    assert!(!piped.success, "{}", piped.stdout);
    assert!(piped.stderr.contains("--yes"), "{}", piped.stderr);
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
}

#[test]
fn learn_dependency_prompt_accept_installs_closure() {
    // spec: DEP-31
    // Answering "y" to the prompt (without --yes) confirms: the whole closure
    // installs, both the selected skill and its pulled-in dependency.
    let sb = dep_fixture();
    let r = sb.mind_tty(&["learn", "skill:review"], "y\n");
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should render before the prompt: {}",
        r.stdout
    );

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed on confirm: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "dependency installed on confirm: {recall}"
    );
}

#[test]
fn learn_pulls_dependency_referenced_in_non_skill_md_file() {
    // spec: DEP-1
    // The dependency scan covers the WHOLE skill directory (matching NS-20's
    // breadth), not just SKILL.md. A `{{ns:reviewer}}` token living in a sibling
    // file (extra.md) inside the skill dir still pulls in the agent.
    let sb = Sandbox::bare("nonmd-deps");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );
    sb.write_and_commit(
        "skills/review/extra.md",
        "see {{ns:reviewer}} for handoff\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviews changes\n---\n# reviewer agent\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "token in a non-SKILL.md file still pulls the dependency: {recall}"
    );
}

#[test]
fn learn_ignores_ns_token_in_a_non_markdown_file_within_the_skill() {
    // spec: DEP-1
    // A `{{ns:reviewer}}` token in a non-markdown sibling file (a shell script)
    // forms no dependency edge: install never expands a token there either
    // (NS-53), so the dependency scan must not treat it as a reference. A
    // partial `learn skill:review` therefore installs ONLY the skill, with no
    // dependency prompt (nothing was found to pull in).
    let sb = Sandbox::bare("nonmd-no-deps");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );
    sb.write_and_commit(
        "skills/review/run.sh",
        "#!/bin/sh\n# see {{ns:reviewer}} for handoff\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviews changes\n---\n# reviewer agent\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // No --yes and no piped input: if the token were (wrongly) treated as a
    // dependency, the confirmation prompt would block on EOF and default to
    // "no", leaving the skill uninstalled -- so the assertions below also
    // pin that no dependency was found at all.
    let r = sb.mind(&["learn", "skill:review"]);
    assert!(r.success, "{}", r.stderr);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review    installed"),
        "the skill must install with no dependency prompt in the way: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer  available"),
        "a {{{{ns:}}}} token in a non-markdown file must not pull in the \
         sibling it names, so the agent stays uninstalled: {recall}"
    );
}

#[test]
fn learn_dependency_already_installed_prompts_but_reinstalls_only_new() {
    // spec: DEP-23 DEP-31
    // Install the dependency alone first. A later partial `learn skill:review`
    // still shows the closure (so it still prompts, the dependency is part of
    // the tree) but the already-installed reviewer is marked [installed] and is
    // not reinstalled; only the new `review` installs.
    let sb = dep_fixture();
    assert!(sb.mind(&["learn", "agent:reviewer", "--yes"]).success);

    let r = sb.mind_tty(&["learn", "skill:review"], "y\n");
    assert!(r.success, "{}", r.stderr);
    // The tree marks the dependency as already installed (DEP-23).
    assert!(
        r.stdout.contains("agent:reviewer [installed]"),
        "already-installed dep should be marked [installed]: {}",
        r.stdout
    );
    // Only the new item is (re)installed; reviewer is not learned again.
    assert!(
        r.stdout.contains("learned skill:review "),
        "the new skill installs: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("learned agent:reviewer "),
        "the already-installed dependency is not reinstalled: {}",
        r.stdout
    );

    // Exactly one reviewer in the manifest (not duplicated), plus the skill.
    let recall = sb.mind(&["recall"]).stdout;
    assert_eq!(
        recall.matches("agent:reviewer").count(),
        1,
        "reviewer must not be duplicated: {recall}"
    );
    assert!(recall.contains("skill:review"), "{recall}");
}

#[test]
fn learn_closure_collision_via_pulled_dependency_aborts() {
    // spec: DEP-30
    // The collision check runs over the FULL closure, not just the explicit
    // selection. Two sources each carry a skill that references its own
    // `{{ns:reviewer}}` agent. Selecting `skill:*` selects two non-colliding
    // skills, but the closure pulls in BOTH `agent:reviewer` items, which
    // collide on `agent:reviewer`. Learn must report the collision and install
    // nothing.
    let a = Sandbox::bare("coll-a");
    a.write_and_commit(
        "skills/areview/SKILL.md",
        "---\nname: areview\ndescription: A review\n---\n# areview\nuse {{ns:reviewer}}\n",
    );
    a.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: A reviewer\n---\n# reviewer\n",
    );
    let b = Sandbox::bare("coll-b");
    b.write_and_commit(
        "skills/breview/SKILL.md",
        "---\nname: breview\ndescription: B review\n---\n# breview\nuse {{ns:reviewer}}\n",
    );
    b.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: B reviewer\n---\n# reviewer\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // The explicit selection (two distinct skills) does not collide; the
    // collision only arises once the pulled-in reviewers join the closure.
    let r = a.mind(&["learn", "skill:*", "--yes"]);
    assert!(!r.success, "closure collision should abort: {}", r.stdout);
    assert!(
        r.stderr.contains("ambiguous"),
        "collision should be reported as ambiguous: {}",
        r.stderr
    );
    // Nothing installed.
    assert!(!a.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn learn_requires_frontmatter_pulls_dependency_closure() {
    // spec: DEP-4 DEP-5
    // A `requires: agent:reviewer` entry in SKILL.md (no {{ns:}} token in the
    // text) still pulls the referenced agent into the dependency closure when
    // `learn` selects the skill alone. The agent installs before the skill
    // (dependency-first order, DEP-21).
    let sb = Sandbox::bare("req-closure");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "learn must succeed: {}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill must be installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "requires entry must pull the dependency into the closure: {recall}"
    );

    // Dependency-first: the reviewer learned line must precede the review line.
    let dep_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned agent:reviewer "));
    let dep_line = dep_line.unwrap_or_else(|| panic!("no reviewer learned line: {}", r.stdout));
    let skill_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned skill:review "));
    let skill_line = skill_line.unwrap_or_else(|| panic!("no review learned line: {}", r.stdout));
    assert!(
        dep_line < skill_line,
        "requires dep must install before its dependent: {}",
        r.stdout
    );
}

#[test]
fn learn_requires_union_with_token_dep_deduped() {
    // spec: DEP-4
    // When the item both declares `requires: agent:reviewer` AND has a
    // {{ns:reviewer}} token in the text, only one dep edge exists: the agent
    // installs exactly once. Regression guard for the dedup invariant.
    let sb = Sandbox::bare("req-dedup");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\nhandoff to {{ns:reviewer}}\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    // Add a third item so the source is a proper subset on `learn skill:review`.
    sb.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    // agent:reviewer appears in the learned output exactly once.
    let reviewer_count = r
        .stdout
        .lines()
        .filter(|l| l.contains("agent:reviewer"))
        .count();
    assert_eq!(
        reviewer_count, 1,
        "dedup: agent:reviewer must appear once in the install output: {}",
        r.stdout
    );
}

#[test]
fn learn_requires_typo_is_bad_reference_error() {
    // spec: DEP-6
    // A `requires:` entry naming a non-existent sibling is a BadReference at
    // install time: `learn` must fail with a non-zero exit and a message
    // referencing the bad entry.
    let sb = Sandbox::bare("req-typo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:nonexistent\n---\n# review skill\n",
    );
    // Add another item so learn sees a proper subset (triggers full validation).
    sb.write_and_commit(
        "agents/helper.md",
        "---\ndescription: helper\n---\n# helper\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(
        !r.success,
        "learn with unresolved requires must fail: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("nonexistent")
            || combined.contains("bad")
            || combined.contains("reference"),
        "error output must mention the bad entry: {combined}"
    );
}

#[test]
fn learn_requires_bad_reference_names_specific_cause() {
    // spec: DEP-7
    // The install-time BadReference for a bad `requires` entry names its specific
    // cause, not the blanket "does not match any item": a source-qualified entry
    // says it crosses sources, and a bare name ambiguous across kinds says so.

    // Cross-source: an `owner/repo#...` entry is forbidden (DEP-5) and must read
    // as crossing sources, not as a miss.
    let xs = Sandbox::bare("req-crosssource");
    xs.write_and_commit(
        "skills/main/SKILL.md",
        "---\nname: main\ndescription: Main\nrequires: other/repo#agent:helper\n---\n# main\n",
    );
    xs.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    assert!(xs.mind(&["meld", &xs.source_spec()]).success, "meld failed");
    let r = xs.mind(&["learn", "skill:main", "--yes"]);
    assert!(
        !r.success,
        "cross-source requires must fail: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("crosses sources"),
        "must name the cross-source cause, not a blanket miss: {combined}"
    );

    // Ambiguous across kinds: a bare name matching both a skill and an agent with
    // no `kind:` qualifier must read as ambiguous, not as a miss.
    let am = Sandbox::bare("req-ambiguous");
    am.write_and_commit(
        "skills/main/SKILL.md",
        "---\nname: main\ndescription: Main\nrequires: dup\n---\n# main\n",
    );
    am.write_and_commit(
        "skills/dup/SKILL.md",
        "---\nname: dup\ndescription: Dup skill\n---\n# dup\n",
    );
    am.write_and_commit(
        "agents/dup.md",
        "---\nname: dup\ndescription: Dup agent\n---\n# dup\n",
    );
    assert!(am.mind(&["meld", &am.source_spec()]).success, "meld failed");
    let r = am.mind(&["learn", "skill:main", "--yes"]);
    assert!(
        !r.success,
        "ambiguous requires must fail: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("ambiguous across kinds"),
        "must name the ambiguity cause, not a blanket miss: {combined}"
    );
}

#[test]
fn learn_requires_resolves_against_own_source_not_a_sibling_source() {
    // spec: DEP-5 DEP-6
    // A `requires` entry is intra-source (DEP-5): it must resolve against the
    // referencing item's OWN source, never another melded source's items. Two
    // sources are melded; source A's skill requires `agent:helper` and that agent
    // exists ONLY in source B (not in A). If validation/resolution leaked across
    // sources, the entry would wrongly resolve and `learn` would succeed. It must
    // instead fail as a BadReference, because A has no `helper` of its own.
    let a = Sandbox::bare("alpha");
    a.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:helper\n---\n# review\n",
    );
    // A has no `helper`; it has only an unrelated sibling so the source is a
    // proper subset on `learn skill:review` (full closure validation runs).
    a.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    // Source B is where a `helper` agent actually lives.
    let b = Sandbox::bare("beta");
    b.write_and_commit(
        "agents/helper.md",
        "---\nname: helper\ndescription: Helper\n---\n# helper\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld A failed");
    assert!(a.mind(&["meld", &b.source_spec()]).success, "meld B failed");

    let r = a.mind(&["learn", "skill:review", "--yes"]);
    assert!(
        !r.success,
        "a requires entry must not resolve against another source's agent: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("helper") || combined.contains("bad") || combined.contains("reference"),
        "error must name the unresolved cross-source entry: {combined}"
    );
}

#[test]
fn review_requires_resolves_per_source_in_a_multi_source_registry() {
    // spec: DEP-5 DEP-6 CLI-131
    // Reviewing a melded source resolves each item's `requires` against that
    // source's own siblings only. Source B is melded and carries `agent:helper`;
    // source A's skill requires `agent:helper` but A has no such agent. Reviewing
    // A (by registry selector) must report the unresolved entry as a hard
    // finding -- B's `helper` must NOT satisfy A's requirement.
    let a = Sandbox::bare("alpha");
    a.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:helper\n---\n# review\n",
    );
    let b = Sandbox::bare("beta");
    b.write_and_commit(
        "agents/helper.md",
        "---\nname: helper\ndescription: Helper\n---\n# helper\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld A failed");
    assert!(a.mind(&["meld", &b.source_spec()]).success, "meld B failed");

    // Review the alpha source by its registry selector.
    let r = a.mind(&["review", "alpha"]);
    assert!(
        !r.success,
        "review of alpha must fail: its requires must not resolve against beta: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("bad-reference") && combined.contains("helper"),
        "must report alpha's unresolved cross-source requires: {combined}"
    );
}

#[test]
fn review_requires_typo_is_hard_finding() {
    // spec: DEP-6 CLI-131
    // `review` surfaces an unresolved `requires:` entry as a hard bad-reference
    // finding, identical in severity to an unresolved {{ns:}} token.
    let sb = Sandbox::bare("review-req-typo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:nonexistent\n---\n# review skill\n",
    );
    // No `agent:nonexistent` item exists in the source.

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        !r.success,
        "review with unresolved requires must exit non-zero: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("bad-reference"),
        "must report a bad-reference finding: {combined}"
    );
    assert!(
        combined.contains("nonexistent"),
        "bad-reference message must name the offending entry: {combined}"
    );
}

// ---------------------------------------------------------------------------
// DEP-60: forget warns about installed dependents
// ---------------------------------------------------------------------------

/// Build a sandbox where `skill:review` depends on `agent:reviewer` (via
/// `requires:`) and both are installed. Returns the sandbox and the source
/// identity string used for meld.
fn dep60_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep60-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    // Install both items (--yes bypasses the dependency prompt).
    assert!(
        sb.mind(&["learn", "skill:review", "--yes"]).success,
        "fixture: learn should succeed"
    );
    sb
}

#[test]
fn forget_single_item_with_dependents_refuses_non_tty_without_force() {
    // spec: DEP-60
    // Forgetting an item that another installed item depends on: in a non-TTY
    // run without --force or --yes, the command must refuse (ConfirmationRequired)
    // and leave the item installed.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "agent:reviewer"]);
    assert!(
        !r.success,
        "forget of a depended-on item must refuse in non-TTY: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );
    // The item was NOT removed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "the item must still be installed after refused forget"
    );
}

#[test]
fn forget_single_item_with_dependents_lists_them() {
    // spec: DEP-60
    // The refusal output must list which installed item(s) depend on the target.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "agent:reviewer"]);
    assert!(!r.success);
    assert!(
        r.stdout.contains("skill:review"),
        "output must name the dependent: {}",
        r.stdout
    );
}

#[test]
fn forget_single_item_with_dependents_proceeds_with_yes() {
    // spec: DEP-60
    // `--yes` (global bypass) lets the removal proceed even when dependents exist.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "--yes", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --yes must proceed: {} {}",
        r.stdout, r.stderr
    );
    // The item is now removed.
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "item must be removed after forget --yes"
    );
    // DEP-50: the dependent is NOT removed (no cascade).
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "dependent must remain installed (DEP-50)"
    );
}

#[test]
fn forget_single_item_with_dependents_proceeds_with_force() {
    // spec: DEP-60
    // `--force` also bypasses the dependents gate; the item is removed.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "--force", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --force must proceed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "item must be removed after forget --force"
    );
    // DEP-50: the dependent is NOT removed.
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "dependent must remain installed (DEP-50)"
    );
}

#[test]
fn forget_single_item_no_dependents_removes_without_extra_prompt() {
    // spec: DEP-60
    // An item with no installed dependents removes immediately with no extra
    // confirmation (CLI-40 behavior unchanged).
    let sb = dep60_fixture();

    // Forget the skill (the dependent, not the dependency) -- no dependents of skill:review.
    let r = sb.mind(&["forget", "skill:review"]);
    assert!(
        r.success,
        "forget with no dependents must not prompt: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "skill must be removed"
    );
    // agent:reviewer is still installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer must remain installed"
    );
}

#[test]
fn forget_glob_path_uses_existing_cli42_confirmation_not_dep60() {
    // spec: DEP-60 CLI-42
    // The glob path (keys.len() > 1) keeps the CLI-42 confirmation unchanged;
    // the DEP-60 dependents gate is single-item only.
    let sb = dep60_fixture();

    // forget '*' hits both items. In a non-TTY without --yes it should refuse
    // with the existing CLI-42 message (count-based), not a DEP-60 dependents
    // warning. We check that it refuses and mentions the count.
    let r = sb.mind(&["forget", "*"]);
    assert!(!r.success, "multi-item forget must refuse: {}", r.stderr);
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("would remove"),
        "must show CLI-42 count message: {}",
        r.stdout
    );
    // Both items must still be installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer still installed"
    );
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "review still installed"
    );
}

// ---------------------------------------------------------------------------
// DEP-61: recall --tree
// ---------------------------------------------------------------------------

/// Fixture for tree tests: a chain `skill:review -> agent:reviewer`.
fn dep61_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep61-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "skill:review", "--yes"]).success,
        "fixture: learn should succeed"
    );
    sb
}

#[test]
fn recall_tree_renders_dependency_forest() {
    // spec: DEP-61
    // `recall --tree` with no item should render the full installed forest.
    // `skill:review` (no incoming installed edge) is a root; `agent:reviewer`
    // is its dependency, nested beneath it.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(
        r.success,
        "recall --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:review"),
        "forest must include skill:review: {out}"
    );
    assert!(
        out.contains("agent:reviewer"),
        "forest must include agent:reviewer: {out}"
    );
    // skill:review is a root (at the start of a line after "- ").
    assert!(
        out.lines().any(|l| l.starts_with("- skill:review")),
        "skill:review must be a root: {out}"
    );
    // agent:reviewer is indented (not a primary root since skill:review depends on it).
    assert!(
        out.lines().any(|l| l.starts_with("  - agent:reviewer")),
        "agent:reviewer must be nested under skill:review: {out}"
    );
}

#[test]
fn recall_tree_item_scopes_to_subtree() {
    // spec: DEP-61
    // `recall <item> --tree` scopes the output to one item's subtree.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "skill:review", "--tree"]);
    assert!(
        r.success,
        "recall <item> --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    // The root of the subtree is the requested item.
    assert!(
        out.lines().any(|l| l.starts_with("- skill:review")),
        "subtree root must be skill:review: {out}"
    );
    // Its dependency appears nested beneath it.
    assert!(
        out.contains("agent:reviewer"),
        "subtree must include the dependency: {out}"
    );
}

#[test]
fn recall_tree_dependency_only_item_is_not_a_root() {
    // spec: DEP-61
    // An item reachable only as a dependency of another installed item must NOT
    // appear as a primary root in the forest; it appears only nested under its
    // dependent.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(r.success, "recall --tree must succeed");
    let out = &r.stdout;
    // agent:reviewer must not appear as a top-level root line.
    assert!(
        !out.lines().any(|l| l.starts_with("- agent:reviewer")),
        "agent:reviewer must not be a primary root: {out}"
    );
}

// ---------------------------------------------------------------------------
// DEP-62: non-interactive probe shows the tree
// ---------------------------------------------------------------------------

/// Fixture for probe tree tests: same `skill:review -> agent:reviewer` chain.
fn dep62_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep62-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn probe_non_interactive_nests_dependency_under_dependent() {
    // spec: DEP-62
    // Non-interactive `probe -n` (no TUI) nests each hit's transitive
    // dependencies beneath it in the human listing. For the dep62 fixture,
    // `skill:review` depends on `agent:reviewer`, so the reviewer line must
    // appear indented as a child of the review hit row.
    // We query "skill:" to match only skill:review (not agent:reviewer), so
    // agent:reviewer appears only as a nested dependency, not as its own hit.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--no-tui", "--kind", "skill", "review"]);
    assert!(
        r.success,
        "probe --no-tui --kind skill must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:review"),
        "skill:review must appear in output: {out}"
    );
    // The dependency agent:reviewer must appear nested (indented) after skill:review.
    let review_pos = out.lines().position(|l| l.contains("skill:review"));
    let reviewer_pos = out.lines().position(|l| l.contains("agent:reviewer"));
    assert!(review_pos.is_some(), "skill:review must appear: {out}");
    assert!(
        reviewer_pos.is_some(),
        "agent:reviewer dependency must appear in output: {out}"
    );
    assert!(
        reviewer_pos.unwrap() > review_pos.unwrap(),
        "agent:reviewer (dependency) must come after skill:review: {out}"
    );
    // The dependency line must be indented (nested under the hit).
    let reviewer_line = out.lines().find(|l| l.contains("agent:reviewer")).unwrap();
    assert!(
        reviewer_line.starts_with("  "),
        "dependency line must be indented: {reviewer_line:?}"
    );
}

#[test]
fn probe_json_includes_dependencies_field() {
    // spec: DEP-62, CLI-167
    // `probe --json` adds a `dependencies` field to each row with the direct
    // dependency keys. For `skill:review` that depends on `agent:reviewer`, the
    // field must contain `"agent:reviewer"`. Output is wrapped in an envelope.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--json", "review"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "review")
        .expect("skill:review must be in JSON output");
    let deps = review_row["dependencies"]
        .as_array()
        .expect("dependencies must be an array");
    assert!(
        deps.iter().any(|d| d == "agent:reviewer"),
        "dependencies must include agent:reviewer: {deps:?}"
    );
}

#[test]
fn probe_json_item_with_no_deps_omits_dependencies_field() {
    // spec: DEP-62, CLI-167
    // An item with no dependencies should have the `dependencies` field absent
    // (or empty) from its JSON row. Output is wrapped in an envelope.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--json", "reviewer"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    let reviewer_row = rows
        .iter()
        .find(|row| row["name"] == "reviewer")
        .expect("agent:reviewer must be in JSON output");
    // Field must be absent (omitted when empty) or present but empty.
    let deps = reviewer_row.get("dependencies");
    assert!(
        deps.is_none() || deps.unwrap().as_array().is_some_and(|a| a.is_empty()),
        "dependencies field must be absent or empty for an item with no deps: {reviewer_row}"
    );
}

// ---------------------------------------------------------------------------
// DEP-60/61/62: additional adversarial / edge coverage (certification shard)
// ---------------------------------------------------------------------------

/// Fixture: a transitive chain `skill:a -> agent:b -> rule:c`, all installed.
/// Each edge is declared with `requires:`. The whole source is melded and
/// installed via a full-coverage glob so resolution is a no-op (DEP-10) and all
/// three land in the manifest regardless of prompting.
fn dep_chain_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep-chain");
    sb.write_and_commit(
        "skills/a/SKILL.md",
        "---\nname: a\ndescription: A\nrequires: agent:b\n---\n# a skill\n",
    );
    sb.write_and_commit(
        "agents/b.md",
        "---\nname: b\ndescription: B\nrequires: rule:c\n---\n# b agent\n",
    );
    sb.write_and_commit(
        "rules/c.md",
        "---\nname: c\ndescription: C\n---\n# c rule\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    // Full-coverage glob installs all three (DEP-10 no-op, no prompt).
    assert!(
        sb.mind(&["learn", "dep-chain#*"]).success,
        "fixture: whole-source learn should install all three"
    );
    sb
}

#[test]
fn forget_transitive_lists_only_direct_dependent_and_no_cascade() {
    // spec: DEP-60 DEP-50
    // Chain a -> b -> c. Forgetting the middle item b: only its DIRECT dependent
    // (a) is listed (c is b's dependency, not a dependent, so it is not listed),
    // the non-TTY run refuses without --yes/--force, and with --yes b is removed
    // while BOTH a (the dependent, no cascade up) and c (b's own dependency, no
    // cascade down, DEP-50) remain installed.
    let sb = dep_chain_fixture();

    // Non-TTY without confirmation: refuse, list only the direct dependent a.
    let refused = sb.mind(&["forget", "agent:b"]);
    assert!(
        !refused.success,
        "forget of a depended-on middle item must refuse: {} {}",
        refused.stdout, refused.stderr
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        refused.stderr
    );
    // The dependent a is listed.
    assert!(
        refused.stdout.contains("skill:a"),
        "the direct dependent skill:a must be listed: {}",
        refused.stdout
    );
    // c is b's dependency, NOT a dependent: it must not appear in the warning's
    // dependent list. (The warning only enumerates dependents.)
    assert!(
        !refused.stdout.lines().any(|l| l.trim() == "rule:c"),
        "rule:c (a dependency of b, not a dependent) must not be listed as a dependent: {}",
        refused.stdout
    );
    // Nothing removed on refusal.
    assert!(sb.mind(&["recall", "agent:b"]).success, "b still installed");

    // With --yes: b is removed; a and c both remain (no cascade either way).
    let done = sb.mind(&["forget", "--yes", "agent:b"]);
    assert!(done.success, "forget --yes must proceed: {}", done.stderr);
    assert!(
        !sb.mind(&["recall", "agent:b"]).success,
        "b must be removed"
    );
    assert!(
        sb.mind(&["recall", "skill:a"]).success,
        "dependent a must remain (no upward cascade)"
    );
    assert!(
        sb.mind(&["recall", "rule:c"]).success,
        "dependency c must remain (no downward cascade, DEP-50)"
    );
}

#[test]
fn forget_dependent_warning_fires_on_union_of_requires_and_token_edges() {
    // spec: DEP-60 DEP-4
    // Two distinct installed items depend on agent:target: one via a `requires:`
    // entry, the other via a `{{ns:}}` token in its body. Forgetting the target
    // must list BOTH dependents -- the dependent set is the union of requires and
    // token edges, not just one source of edge.
    let sb = Sandbox::bare("dep-union");
    sb.write_and_commit(
        "agents/target.md",
        "---\nname: target\ndescription: Target\n---\n# target\n",
    );
    // Dependent via requires:.
    sb.write_and_commit(
        "skills/via-requires/SKILL.md",
        "---\nname: via-requires\ndescription: R\nrequires: agent:target\n---\n# via-requires\n",
    );
    // Dependent via a {{ns:}} token in the body.
    sb.write_and_commit(
        "skills/via-token/SKILL.md",
        "---\nname: via-token\ndescription: T\n---\n# via-token\nhand off to {{ns:target}}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "dep-union#*"]).success,
        "fixture: whole-source learn should install all three"
    );

    let r = sb.mind(&["forget", "agent:target"]);
    assert!(
        !r.success,
        "forget of the doubly-depended item must refuse: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skill:via-requires"),
        "the requires-edge dependent must be listed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:via-token"),
        "the token-edge dependent must be listed: {}",
        r.stdout
    );
}

#[test]
fn forget_force_does_not_bypass_cli42_multi_item_confirmation() {
    // spec: DEP-60 CLI-42
    // `--force` bypasses only the DEP-60 single-item dependents gate. A glob that
    // matches 2+ items still routes through the CLI-42 multi-item confirmation,
    // which only `--yes` bypasses. So a non-TTY `forget --force '*'` over 2+
    // matches must STILL refuse (ConfirmationRequired) and remove nothing.
    let sb = dep60_fixture(); // two installed items: skill:review, agent:reviewer

    let r = sb.mind(&["forget", "--force", "*"]);
    assert!(
        !r.success,
        "forget --force over a multi-match glob must still refuse: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired (CLI-42, not bypassed by --force): {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("would remove"),
        "must show the CLI-42 count message: {}",
        r.stdout
    );
    // Both items remain.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer must remain installed"
    );
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "review must remain installed"
    );
}

#[test]
fn recall_tree_item_with_no_dependencies_prints_just_that_item() {
    // spec: DEP-61
    // `recall <item> --tree` for an item that has no dependencies prints exactly
    // that one item as the subtree root, with no nested children.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    // agent:reviewer is a leaf (it depends on nothing).
    let r = sb.mind(&["recall", "agent:reviewer", "--tree"]);
    assert!(
        r.success,
        "recall <leaf> --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let lines: Vec<&str> = r.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["- agent:reviewer"],
        "a dependency-free item's subtree must be just that item: {:?}",
        r.stdout
    );
}

/// Fixture: a 2-cycle among installed items. `skill:loop-a` and `skill:loop-b`
/// each `requires:` the other; the whole source is installed so both land.
fn dep_cycle_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep-cycle");
    sb.write_and_commit(
        "skills/loop-a/SKILL.md",
        "---\nname: loop-a\ndescription: A\nrequires: skill:loop-b\n---\n# loop-a\n",
    );
    sb.write_and_commit(
        "skills/loop-b/SKILL.md",
        "---\nname: loop-b\ndescription: B\nrequires: skill:loop-a\n---\n# loop-b\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "dep-cycle#*"]).success,
        "fixture: whole-source learn should install both cycle members"
    );
    sb
}

#[test]
fn recall_tree_cyclic_installed_pair_renders_every_item() {
    // spec: DEP-61 DEP-22
    // A pure cycle among installed items (every node has in-degree >= 1, so no
    // natural root exists) must still render EVERY installed item in the forest:
    // a secondary root is promoted and the back-edge is marked (cycle). No
    // installed item may be missing from `recall --tree` output.
    let sb = dep_cycle_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(
        r.success,
        "recall --tree over a cycle must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:loop-a"),
        "loop-a must appear in the forest: {out}"
    );
    assert!(
        out.contains("skill:loop-b"),
        "loop-b must appear in the forest: {out}"
    );
    // The cycle must be broken with a marked back-edge, not expanded forever.
    assert!(
        out.contains("(cycle)"),
        "the cycle must be rendered as a marked back-edge: {out}"
    );
}

#[test]
fn probe_json_resolves_dependency_to_prefixed_effective_key() {
    // spec: DEP-62, CLI-167
    // When a source is melded under a prefix, an item's dependency key in the
    // `probe --json` adjacency field must be the EFFECTIVE (prefixed) key.
    // Output is wrapped in an envelope.
    let sb = Sandbox::bare("dep-prefix");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec(), "--as", "jk"]).success);

    let r = sb.mind(&["probe", "--json", "review"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    // The effective name carries the prefix.
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "jk:review")
        .expect("skill:jk:review must be in JSON output (prefixed effective name)");
    let deps = review_row["dependencies"]
        .as_array()
        .expect("dependencies must be an array");
    assert!(
        deps.iter().any(|d| d == "agent:jk:reviewer"),
        "dependency key must be the prefixed effective key agent:jk:reviewer, not bare: {deps:?}"
    );
    assert!(
        !deps.iter().any(|d| d == "agent:reviewer"),
        "the bare (unprefixed) dependency key must NOT appear: {deps:?}"
    );
}

// DEP-63: recall --tree --json structured output
// ---------------------------------------------------------------------------

#[test]
fn recall_tree_json_emits_json_array_with_dependency_nested() {
    // spec: DEP-63
    // `recall --tree --json` emits a JSON array of root nodes.
    // Fixture: skill:review -> agent:reviewer.
    // skill:review has in-degree 0 (it is the root); agent:reviewer is its
    // dependency.  The root node must have "key": "skill:review" and its
    // "dependencies" must contain one entry "agent:reviewer".
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Output must be valid JSON.
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    // Top-level is a JSON array (the forest roots).
    assert!(v.is_array(), "output must be a JSON array: {v}");
    let arr = v.as_array().unwrap();

    // Exactly one root: skill:review.
    let root = arr
        .iter()
        .find(|n| n["key"] == "skill:review")
        .unwrap_or_else(|| panic!("must have skill:review as root: {arr:?}"));

    // Normal node: has "dependencies", no "cycle".
    assert!(
        root.get("dependencies").is_some(),
        "root node must have dependencies field: {root}"
    );
    assert!(
        root.get("cycle").is_none(),
        "root node must not have cycle field: {root}"
    );

    // agent:reviewer is nested under skill:review.
    let deps = root["dependencies"].as_array().unwrap();
    let reviewer = deps
        .iter()
        .find(|n| n["key"] == "agent:reviewer")
        .unwrap_or_else(|| panic!("agent:reviewer must be in dependencies: {deps:?}"));
    assert!(
        reviewer.get("cycle").is_none(),
        "reviewer node must not be a cycle: {reviewer}"
    );
    let reviewer_deps = reviewer["dependencies"]
        .as_array()
        .expect("reviewer must have dependencies field");
    assert!(
        reviewer_deps.is_empty(),
        "reviewer is a leaf, so dependencies must be empty: {reviewer_deps:?}"
    );
}

#[test]
fn recall_tree_json_item_emits_single_object_not_array() {
    // spec: DEP-63
    // `recall <item> --tree --json` emits a single JSON object (not an array)
    // for that item's subtree.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "skill:review", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <item> --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    // Must be an object, not an array.
    assert!(
        v.is_object(),
        "scoped recall --tree --json must emit an object: {v}"
    );
    assert_eq!(
        v["key"], "skill:review",
        "object key must be skill:review: {v}"
    );
    let deps = v["dependencies"]
        .as_array()
        .expect("root object must have dependencies");
    assert_eq!(deps.len(), 1, "skill:review has one dependency: {deps:?}");
    assert_eq!(deps[0]["key"], "agent:reviewer");
}

#[test]
fn recall_tree_json_leaf_item_has_empty_dependencies() {
    // spec: DEP-63
    // `recall agent:reviewer --tree --json` for a leaf node emits a single
    // object with an empty `dependencies` array (not absent).
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "agent:reviewer", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <leaf> --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    assert!(v.is_object(), "must be an object: {v}");
    assert_eq!(v["key"], "agent:reviewer");
    let deps = v["dependencies"]
        .as_array()
        .expect("leaf object must have dependencies field (not absent)");
    assert!(
        deps.is_empty(),
        "leaf must have empty dependencies array: {deps:?}"
    );
}

#[test]
fn recall_tree_json_with_prefix_uses_effective_keys() {
    // spec: DEP-63
    // When items are installed under a prefix, the JSON keys must use the
    // effective (prefixed) name, matching what recall --tree (human) emits.
    let sb = Sandbox::bare("dep63-prefix");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    // Meld with a prefix so items install as "pfx:review" / "pfx:reviewer".
    assert!(
        sb.mind(&["meld", "--as", "pfx", &sb.source_spec()]).success,
        "meld with prefix must succeed"
    );
    // Under a prefix, the effective name is "pfx:review" -- use that to learn.
    assert!(
        sb.mind(&["learn", "pfx:review", "--yes"]).success,
        "learn with prefix must succeed"
    );

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json with prefix must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("must be valid JSON: {e}\n{}", r.stdout));
    assert!(v.is_array());
    let arr = v.as_array().unwrap();

    // Root key must use the effective prefixed name.
    let root = arr
        .iter()
        .find(|n| n["key"] == "skill:pfx:review")
        .unwrap_or_else(|| panic!("root must be skill:pfx:review: {arr:?}"));

    let deps = root["dependencies"].as_array().unwrap();
    assert_eq!(deps.len(), 1, "one dep: {deps:?}");
    assert_eq!(
        deps[0]["key"], "agent:pfx:reviewer",
        "dep must use effective prefixed key: {deps:?}"
    );
}

#[test]
fn recall_tree_with_sources_resolves_to_sources_path_with_note() {
    // spec: DEP-61
    // Precedence pin: `recall --tree --sources` is NOT the tree path. The code
    // notes that --tree is ignored with --sources and runs the sources listing.
    // This pins the resolved precedence so it cannot silently change: a note is
    // emitted on stderr AND the output is the sources listing (showing the melded
    // source), not a dependency forest of installed items.
    let sb = dep61_fixture(); // one melded source "dep61-agents", items installed

    let r = sb.mind(&["recall", "--tree", "--sources"]);
    assert!(
        r.success,
        "recall --tree --sources must succeed: {} {}",
        r.stdout, r.stderr
    );
    // The note about --tree being ignored with --sources is emitted.
    assert!(
        r.stderr.contains("--tree") && r.stderr.contains("ignored with --sources"),
        "a note that --tree is ignored with --sources must be emitted: {}",
        r.stderr
    );
    // The sources path ran: the melded source appears in the listing.
    assert!(
        r.stdout.contains("dep61-agents"),
        "the sources listing (not a dependency forest) must be shown: {}",
        r.stdout
    );
    // It is NOT a dependency forest: no nested "- agent:reviewer" tree line.
    assert!(
        !r.stdout
            .lines()
            .any(|l| l.starts_with("  - agent:reviewer")),
        "must not render the dependency forest under --sources: {}",
        r.stdout
    );
}

#[test]
fn recall_tree_json_not_installed_item_errors_like_non_json() {
    // spec: DEP-63
    // `recall <item> --tree --json` for an item that is NOT installed at all must
    // error the same way the non-json `recall <item> --tree` does (NotInstalled
    // via resolve_installed), NOT emit a null/empty object. The DepNode::normal
    // fallback in commands/ is only reached AFTER resolve_installed succeeds
    // (item present in the manifest), so an absent item never reaches it.
    let sb = dep61_fixture(); // skill:review and agent:reviewer installed

    // A skill that exists in the source but was never learned is not installed.
    let json = sb.mind(&["recall", "skill:nope", "--tree", "--json"]);
    assert!(
        !json.success,
        "recall <uninstalled> --tree --json must fail: {} {}",
        json.stdout, json.stderr
    );
    // Under --json the error appears as the CLI-181 envelope on stdout.
    let v = parse_json(&json.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", json.stdout);
    assert_eq!(
        v["error"]["kind"], "not-installed",
        "kind must be not-installed: {}",
        json.stdout
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .map(|s| s.contains("not installed") || s.contains("nope"))
            .unwrap_or(false),
        "message must mention the missing item: {}",
        json.stdout
    );

    // Parity: the non-json form fails the same way.
    let human = sb.mind(&["recall", "skill:nope", "--tree"]);
    assert!(
        !human.success,
        "non-json recall <uninstalled> --tree must also fail: {} {}",
        human.stdout, human.stderr
    );
    assert!(
        human.stderr.contains("not installed"),
        "non-json form must also report NotInstalled: {}",
        human.stderr
    );
}

#[test]
fn recall_tree_json_installed_but_orphaned_item_falls_back_to_normal_node() {
    // spec: DEP-63
    // The `DepNode::normal(key, [])` fallback in commands/: an item that IS in
    // the manifest (resolve_installed succeeds) but is NOT a node in the graph
    // (it was removed upstream, so the catalog no longer carries it, and
    // subtree_node returns None). The scoped `recall <item> --tree --json` must
    // still emit a valid single object {"key": ..., "dependencies": []}, not
    // null and not an error.
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");
    // The agent disappears upstream; sync drops it from the catalog while it
    // stays installed in the manifest.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let r = sb.mind(&["recall", "agent:dev", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <orphaned> --tree --json must succeed via the fallback: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    assert!(
        v.is_object(),
        "fallback must emit a single object, not an array: {v}"
    );
    assert_eq!(
        v["key"], "agent:dev",
        "fallback node key must be the item key: {v}"
    );
    let deps = v["dependencies"]
        .as_array()
        .expect("fallback node must carry an (empty) dependencies array");
    assert!(
        deps.is_empty(),
        "an orphaned item has no graph edges, so dependencies must be empty: {deps:?}"
    );
    assert!(
        v.get("cycle").is_none(),
        "the fallback node must not be a cycle leaf: {v}"
    );
}

#[test]
fn recall_tree_json_empty_manifest_emits_empty_array() {
    // spec: DEP-63
    // `recall --tree --json` with nothing installed must emit a valid JSON empty
    // array `[]`, not an error, not an empty string, not the human "no installed
    // items" line.
    // A melded-but-not-learned source has an empty manifest.
    let sb = dep61_fixture_unlearned();

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json over an empty manifest must succeed: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    assert!(
        v.as_array().is_some_and(|a| a.is_empty()),
        "empty manifest must yield an empty JSON array: {v}"
    );
}

/// Like `dep61_fixture` but the source is melded and NOT learned, so the
/// manifest stays empty (for the empty-forest case).
fn dep61_fixture_unlearned() -> Sandbox {
    let sb = Sandbox::bare("dep63-empty");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn recall_tree_json_cyclic_pair_every_item_present_with_cycle_leaf() {
    // spec: DEP-63 DEP-22
    // A pure cycle among installed items, driven through the real binary: the
    // structured forest must still contain EVERY installed item (the cycle
    // promotes a secondary root), the back-edge is a {"cycle": true} leaf with
    // no "dependencies" field, and there is no infinite nesting (bounded output).
    let sb = dep_cycle_fixture(); // skill:loop-a <-> skill:loop-b, both installed

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json over a cycle must succeed: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    let arr = v.as_array().expect("forest must be a JSON array");

    // Collect every key that appears anywhere in the structured forest.
    fn collect(node: &serde_json::Value, out: &mut std::collections::HashSet<String>) {
        if let Some(k) = node["key"].as_str() {
            out.insert(k.to_string());
        }
        if let Some(children) = node["dependencies"].as_array() {
            for c in children {
                collect(c, out);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for root in arr {
        collect(root, &mut seen);
    }
    assert!(
        seen.contains("skill:loop-a") && seen.contains("skill:loop-b"),
        "every installed cycle member must appear in the JSON forest: {seen:?}"
    );

    // Find at least one cycle leaf: {"cycle": true} with no "dependencies".
    fn has_cycle_leaf(node: &serde_json::Value) -> bool {
        if node["cycle"] == serde_json::Value::Bool(true) {
            assert!(
                node.get("dependencies").is_none(),
                "a cycle leaf must omit dependencies: {node}"
            );
            return true;
        }
        node["dependencies"]
            .as_array()
            .is_some_and(|cs| cs.iter().any(has_cycle_leaf))
    }
    assert!(
        arr.iter().any(has_cycle_leaf),
        "the cycle must surface as a {{cycle:true}} leaf, not infinite nesting: {v}"
    );
}

#[test]
fn recall_tree_json_and_probe_json_agree_on_direct_dependencies() {
    // spec: DEP-63 DEP-62
    // Cross-form consistency: `recall --tree --json` (nested tree, DEP-63) and
    // `probe --json` (flat adjacency, DEP-62) must describe the SAME direct edges
    // for the same item. For skill:review, both must report agent:reviewer as its
    // single direct dependency.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    // Nested tree form: skill:review's direct children.
    let tree = sb.mind(&["recall", "skill:review", "--tree", "--json"]);
    assert!(
        tree.success,
        "recall --tree --json must succeed: {}",
        tree.stderr
    );
    let tv: serde_json::Value = serde_json::from_str(tree.stdout.trim())
        .unwrap_or_else(|e| panic!("recall tree JSON invalid: {e}\n{:?}", tree.stdout));
    let mut tree_deps: Vec<String> = tv["dependencies"]
        .as_array()
        .expect("subtree object must have dependencies")
        .iter()
        .map(|n| n["key"].as_str().unwrap().to_string())
        .collect();
    tree_deps.sort();

    // Flat adjacency form: skill:review's `dependencies` field.
    // spec: CLI-167 - probe --json is wrapped in an envelope.
    let probe = sb.mind(&["probe", "--json", "review"]);
    assert!(probe.success, "probe --json must succeed: {}", probe.stderr);
    let probe_env: serde_json::Value =
        serde_json::from_str(&probe.stdout).expect("probe JSON invalid");
    let rows = probe_env["items"].as_array().expect("items must be array");
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "review")
        .expect("skill:review must be a probe row");
    let mut probe_deps: Vec<String> = review_row
        .get("dependencies")
        .and_then(|d| d.as_array())
        .map(|a| a.iter().map(|d| d.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    probe_deps.sort();

    assert_eq!(
        tree_deps, probe_deps,
        "recall --tree --json and probe --json must agree on skill:review's direct deps"
    );
    assert_eq!(
        tree_deps,
        vec!["agent:reviewer".to_string()],
        "both forms must report exactly agent:reviewer: {tree_deps:?}"
    );
}

// ---------------------------------------------------------------------------
// C3: forget --json without --yes when dependents exist => ConfirmationRequired
// ---------------------------------------------------------------------------

/// Under `--json` without `--yes` or `--force`, forgetting an item that has
/// installed dependents must return ConfirmationRequired and remove nothing.
/// json mode is non-interactive; it must not silently proceed through a
/// destructive confirmation (DEP-60).
#[test]
fn forget_json_without_yes_when_dependents_exist_is_confirmation_required() {
    // spec: DEP-60, CLI-181
    let sb = dep60_fixture(); // skill:review depends on agent:reviewer

    // --json but no --yes and no --force.
    let r = sb.mind(&["--json", "forget", "agent:reviewer"]);
    assert!(
        !r.success,
        "forget --json without --yes must refuse when dependents exist: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // Under --json the ConfirmationRequired error goes to stdout as the CLI-181 envelope.
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert_eq!(
        v["error"]["kind"], "confirmation-required",
        "kind must be confirmation-required: {}",
        r.stdout
    );
    // The item must still be installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "agent:reviewer must remain installed after json refusal"
    );
}

/// `--json --yes` must still proceed (yes overrides the confirmation gate).
#[test]
fn forget_json_with_yes_when_dependents_exist_proceeds() {
    // spec: DEP-60
    let sb = dep60_fixture();

    let r = sb.mind(&["--json", "--yes", "forget", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --json --yes must proceed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "agent:reviewer must be removed after forget --json --yes"
    );
}

// ---------------------------------------------------------------------------
// Consumer pin-flag injection: leading-dash values rejected (DSC-66)
// ---------------------------------------------------------------------------
