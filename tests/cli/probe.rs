//! `probe` and its TUI (spec/cli.md, spec/tui.md).

use super::*;

#[test]
fn probe_lists_all_three_kinds() {
    // spec: CLI-80, DSC-1, DSC-10, DSC-11, DSC-12, DSC-36
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success);
    assert!(r.stdout.contains("skill:review"), "{}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "{}", r.stdout);
    assert!(r.stdout.contains("rule:style"), "{}", r.stdout);
}

#[test]
fn probe_filters_by_substring() {
    // spec: CLI-80
    let sb = melded();
    let r = sb.mind(&["probe", "review"]);
    assert!(r.stdout.contains("skill:review"));
    assert!(!r.stdout.contains("agent:dev"), "{}", r.stdout);
}

#[test]
fn probe_matches_description_text() {
    // spec: CLI-85
    // The fixture's skill:review has description "Review the diff for bugs".
    // Querying "bugs" is only present in the description, not the item name.
    let sb = melded();
    let r = sb.mind(&["probe", "bugs"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("skill:review"),
        "expected skill:review in output: {}",
        r.stdout
    );
    // agent:dev description is "Implements a spec with tests" - no "bugs"
    assert!(
        !r.stdout.contains("agent:dev"),
        "unexpected agent:dev in output: {}",
        r.stdout
    );
}

#[test]
fn probe_query_is_case_insensitive() {
    // spec: CLI-85
    // "Review" (capitalized) matches item name "review" case-insensitively.
    let sb = melded();
    let r = sb.mind(&["probe", "REVIEW"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("skill:review"),
        "expected skill:review in output: {}",
        r.stdout
    );
}

#[test]
fn probe_description_query_composes_with_kind_filter() {
    // spec: CLI-85, CLI-83
    // The agent:dev description contains "spec". Filter to agents only.
    let sb = melded();
    let r = sb.mind(&["probe", "--kind", "agent", "spec"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("agent:dev"),
        "expected agent:dev in output: {}",
        r.stdout
    );
    // skill:review description mentions "diff" not "spec"; should be excluded
    assert!(
        !r.stdout.contains("skill:review"),
        "unexpected skill:review in output: {}",
        r.stdout
    );
}

#[test]
fn probe_description_query_composes_with_source_filter() {
    // spec: CLI-85, CLI-83
    // Implementor flagged this gap: --source composition with a
    // description-only query had no integration test (only --kind did).
    //
    // Meld two sources. The standard fixture (`agents`) describes its review
    // skill as "Review the diff for bugs". A second source (`tools`) carries a
    // review skill whose description has the unique word "kubernetes", absent
    // from every item in `agents`. Querying that word with --source must match
    // the item in `tools` by description and exclude `agents` entirely.
    let agents = melded();
    let tools = Sandbox::named("tools");
    tools.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Deploy onto kubernetes clusters\n---\n# review skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of second source failed"
    );

    // --source tools + the description-only query matches the tools item and
    // names the tools source.
    let in_tools = agents.mind(&["probe", "--source", "tools", "kubernetes"]);
    assert!(in_tools.success, "probe failed: {}", in_tools.stderr);
    assert!(
        in_tools.stdout.contains("skill:review"),
        "expected skill:review from tools: {}",
        in_tools.stdout
    );
    assert!(
        in_tools.stdout.contains("tools"),
        "expected the tools source column: {}",
        in_tools.stdout
    );

    // The same query scoped to the other source matches nothing: "kubernetes"
    // is not in any `agents` item, so the source filter composes (it does not
    // leak across sources).
    let in_agents = agents.mind(&["probe", "--source", "agents", "kubernetes"]);
    assert!(in_agents.success, "probe failed: {}", in_agents.stderr);
    assert!(
        !in_agents.stdout.contains("skill:review"),
        "kubernetes must not match any agents item: {}",
        in_agents.stdout
    );
    assert!(
        in_agents.stdout.contains("no items match"),
        "expected an empty-result note: {}",
        in_agents.stdout
    );
}

#[test]
fn probe_source_glob_narrows_to_matching_sources() {
    // spec: CLI-86 - the `--source` filter accepts a glob matched against source
    // identities; `*agents` shows only the agents source's items and excludes a
    // second `tools` source.
    let (sb, _tools) = melded_two_sources();

    let only_agents = sb.mind(&["probe", "--no-tui", "--source", "*agents"]);
    assert!(only_agents.success, "{}", only_agents.stderr);
    assert!(
        only_agents.stdout.contains("skill:review"),
        "expected the agents source's item: {}",
        only_agents.stdout
    );
    assert!(
        !only_agents.stdout.contains("skill:deploy"),
        "the tools source's item must be excluded: {}",
        only_agents.stdout
    );

    // The complementary glob shows only the tools source's item.
    let only_tools = sb.mind(&["probe", "--no-tui", "--source", "*tools"]);
    assert!(only_tools.success, "{}", only_tools.stderr);
    assert!(
        only_tools.stdout.contains("skill:deploy"),
        "expected the tools source's item: {}",
        only_tools.stdout
    );
    assert!(
        !only_tools.stdout.contains("skill:review"),
        "the agents source's item must be excluded: {}",
        only_tools.stdout
    );
}

#[test]
fn recall_source_glob_narrows_to_matching_sources() {
    // spec: CLI-86 - the `recall` listing `--source` filter accepts a glob the
    // same way as probe.
    let (sb, _tools) = melded_two_sources();

    let only_agents = sb.mind(&["recall", "--source", "*agents"]);
    assert!(only_agents.success, "{}", only_agents.stderr);
    assert!(
        only_agents.stdout.contains("review"),
        "expected the agents source's item: {}",
        only_agents.stdout
    );
    assert!(
        !only_agents.stdout.contains("deploy"),
        "the tools source's item must be excluded: {}",
        only_agents.stdout
    );
}

#[test]
fn probe_source_glob_matching_nothing_is_empty() {
    // spec: CLI-86 - a glob that matches no source yields an empty listing (no
    // error), as any fully-excluding filter does.
    let (sb, _tools) = melded_two_sources();
    let r = sb.mind(&["probe", "--no-tui", "--source", "*nope"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stdout.contains("skill:review") && !r.stdout.contains("skill:deploy"),
        "no items should be listed: {}",
        r.stdout
    );
}

#[test]
fn probe_source_glob_composes_with_json() {
    // spec: CLI-86, CLI-84, CLI-167 - the glob `--source` filter composes with `--json`;
    // result is wrapped in {"schema": 1, "items": [...]}.
    let (sb, _tools) = melded_two_sources();
    let r = sb.mind(&["probe", "--no-tui", "--source", "*agents", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let envelope: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("probe --json envelope");
    assert_eq!(
        envelope["schema"], 1,
        "schema field must be 1: {}",
        r.stdout
    );
    let rows = envelope["items"].as_array().expect("items array");
    assert!(
        rows.iter().any(|row| row["name"] == "review"),
        "agents item present in json: {}",
        r.stdout
    );
    assert!(
        !rows.iter().any(|row| row["name"] == "deploy"),
        "tools item excluded from json: {}",
        r.stdout
    );
}

#[test]
fn probe_source_glob_composes_with_kind_and_query() {
    // spec: CLI-86, CLI-85, CLI-83 - the glob `--source` filter ANDs with `--kind`
    // and the positional substring query simultaneously. Add a non-skill item and a
    // non-matching skill to the agents source so each filter is load-bearing: only
    // the row that satisfies all three (source `*agents`, kind `skill`, query
    // `review`) survives.
    let (sb, _tools) = melded_two_sources();

    let r = sb.mind(&[
        "probe", "--no-tui", "--source", "*agents", "--kind", "skill", "review",
    ]);
    assert!(r.success, "{}", r.stderr);
    // Satisfies all three filters.
    assert!(
        r.stdout.contains("skill:review"),
        "the item matching source+kind+query must be shown: {}",
        r.stdout
    );
    // Excluded by --kind (same source, matches neither kind nor query).
    assert!(
        !r.stdout.contains("rule:style"),
        "--kind must exclude the rule: {}",
        r.stdout
    );
    // Excluded by --kind (an agent in the same source).
    assert!(
        !r.stdout.contains("agent:dev"),
        "--kind must exclude the agent: {}",
        r.stdout
    );
    // Excluded by --source (the tools source's skill, which would pass --kind).
    assert!(
        !r.stdout.contains("skill:deploy"),
        "--source must exclude the other source's skill: {}",
        r.stdout
    );

    // A query that matches no item in the selected source+kind yields nothing.
    let none = sb.mind(&[
        "probe", "--no-tui", "--source", "*agents", "--kind", "skill", "deploy",
    ]);
    assert!(none.success, "{}", none.stderr);
    assert!(
        !none.stdout.contains("skill:"),
        "the query must still exclude non-matching items in the selected source/kind: {}",
        none.stdout
    );
}

#[test]
fn recall_sources_ignores_source_filter_glob() {
    // spec: CLI-83, CLI-86 - the `--source` filter (glob or not) applies to the
    // installed-items listing, NOT to the `--sources` view. Per CLI-83, passing
    // `--source` with `--sources` lists ALL sources and prints a note that the
    // filter is ignored; it does not narrow the source list.
    let (sb, _tools) = melded_two_sources();
    let agents_full = format!("{}/agents", sb.base_name());

    let r = sb.mind(&["recall", "--sources", "--source", "*agents"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // Both sources are still listed: the filter does not narrow `--sources`.
    assert!(
        r.stdout.contains(&agents_full),
        "the agents source must be listed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("/tools"),
        "the non-matching tools source must STILL be listed (filter ignored): {}",
        r.stdout
    );
    // The ignored-filter note is printed (CLI-83).
    assert!(
        r.stderr.contains("ignored with --sources"),
        "a note that the filter is ignored must be printed: {}",
        r.stderr
    );
}

#[test]
fn probe_query_matches_name_in_one_item_and_description_in_another() {
    // spec: CLI-85
    // A single query resolves via the NAME of one item and the DESCRIPTION of
    // another in the same result set. "audit" is the skill's name and also
    // appears only inside the agent's description, so both must be returned.
    let sb = Sandbox::named("dual");
    // skill:audit - "audit" only in the NAME.
    sb.write_and_commit(
        "skills/audit/SKILL.md",
        "---\nname: audit\ndescription: Inspect changes carefully\n---\n# audit\n",
    );
    // agent:dev - "audit" only in the DESCRIPTION, not the name.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Run an audit before shipping\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "audit"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    // Matched by name.
    assert!(
        r.stdout.contains("skill:audit"),
        "expected skill:audit (name match): {}",
        r.stdout
    );
    // Matched by description in a different item.
    assert!(
        r.stdout.contains("agent:dev"),
        "expected agent:dev (description match): {}",
        r.stdout
    );
}

#[test]
fn probe_matches_substring_in_middle_of_word() {
    // spec: CLI-85
    // The match is a raw substring, not a word-boundary match: a query that is
    // a fragment inside a longer word still matches.
    let sb = Sandbox::named("frag");
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Performs refactoring of modules\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // "factor" is in the middle of "refactoring".
    let r = sb.mind(&["probe", "factor"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("agent:dev"),
        "expected mid-word substring match: {}",
        r.stdout
    );
}

#[test]
fn probe_marks_installed_and_shows_hash() {
    // spec: CLI-81
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let probe = sb.mind(&["probe"]).stdout;

    let review = probe.lines().find(|l| l.contains("skill:review")).unwrap();
    assert!(
        review.starts_with('*'),
        "installed item should be marked: {review:?}"
    );
    let dev = probe.lines().find(|l| l.contains("agent:dev")).unwrap();
    assert!(
        !dev.starts_with('*'),
        "uninstalled item should not be marked: {dev:?}"
    );

    // A short (8 hex) content hash appears on the row.
    assert!(
        review
            .split_whitespace()
            .any(|t| t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit())),
        "expected a short hash: {review:?}"
    );
}

#[test]
fn probe_columns_align_with_long_names() {
    // spec: CLI-82
    let sb = Sandbox::new();
    // A key longer than the old fixed width, to exercise dynamic column sizing.
    sb.write_and_commit(
        "skills/consumer-experience-review/SKILL.md",
        "---\ndescription: long-named skill\n---\n# x\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let probe = sb.mind(&["probe"]).stdout;
    let cols: Vec<usize> = probe
        .lines()
        .filter(|l| l.contains("/agents"))
        .map(|l| l.find("local/").expect("source column on every row"))
        .collect();
    assert!(cols.len() >= 2, "expected several rows: {probe}");
    assert!(
        cols.iter().all(|&c| c == cols[0]),
        "source column misaligned: {cols:?}\n{probe}"
    );
}

#[test]
fn probe_and_recall_filter_by_kind_and_source() {
    // spec: CLI-83
    let sb = melded();

    // probe --kind narrows to one kind, composing with the substring query.
    let skills = sb.mind(&["probe", "--kind", "skill"]).stdout;
    assert!(skills.contains("skill:review"), "{skills}");
    assert!(!skills.contains("agent:dev"), "{skills}");

    // probe --source narrows by source selector (the repo basename suffix).
    let by_source = sb.mind(&["probe", "--source", "agents"]).stdout;
    assert!(by_source.contains("skill:review"), "{by_source}");
    let no_source = sb.mind(&["probe", "--source", "nope"]).stdout;
    assert!(!no_source.contains("skill:review"), "{no_source}");

    // recall --kind filters the installed listing.
    assert!(sb.mind(&["learn", "*"]).success);
    let only_agents = sb.mind(&["recall", "--kind", "agent"]).stdout;
    assert!(only_agents.contains("agent:dev"), "{only_agents}");
    assert!(!only_agents.contains("skill:review"), "{only_agents}");

    // Filters are meaningless with --sources; recall says so rather than
    // silently ignoring them.
    let warned = sb.mind(&["recall", "--sources", "--kind", "skill"]);
    assert!(warned.success, "{}", warned.stderr);
    assert!(warned.stderr.contains("ignored"), "{}", warned.stderr);
}

#[test]
fn probe_json_emits_rows() {
    // spec: CLI-84, CLI-167 - output is wrapped in {"schema":1,"items":[...]}.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("probe --json envelope");
    assert_eq!(env["schema"], 1, "schema must be 1: {}", r.stdout);
    let rows = env["items"].as_array().expect("items must be array");
    assert!(r.stdout.contains("\"installed\""), "{}", r.stdout);
    assert!(r.stdout.contains("\"name\": \"review\""), "{}", r.stdout);
    // The installed item carries installed:true.
    assert!(
        rows.iter().any(|row| row["installed"] == true),
        "{}",
        r.stdout
    );
}

// --- unmanaged lobe items (spec/unmanaged.md) -------------------------------

#[test]
fn probe_fallback_on_non_tty_stdout_produces_listing() {
    // spec: TUI-2
    // The test harness pipes stdout, so is_terminal() returns false; probe must
    // fall back to the plain catalog listing rather than entering raw mode.
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success, "probe fallback should succeed: {}", r.stderr);
    // Listing shows all three kinds.
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("rule:style"), "listing: {}", r.stdout);
    // No ANSI raw-mode escape sequences (the listing does not use ratatui).
    assert!(
        !r.stdout.contains("\x1b[?1049h"),
        "raw-mode alt-screen escape must not appear in fallback output"
    );
}

#[test]
fn probe_no_tui_flag_produces_listing() {
    // spec: TUI-2 - `--no-tui` forces the plain listing even on a TTY.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe --no-tui should succeed: {}", r.stderr);
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "listing: {}", r.stdout);
}

#[test]
fn probe_json_flag_produces_json_not_tui() {
    // spec: TUI-2, CLI-167 - `--json` forces JSON output wrapped in envelope.
    let sb = melded();
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "probe --json should succeed: {}", r.stderr);
    let env: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("probe --json must produce valid JSON");
    assert_eq!(
        env["schema"], 1,
        "probe --json must produce envelope: {}",
        r.stdout
    );
    assert!(
        env["items"].is_array(),
        "probe --json envelope must have items array: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("\x1b[?1049h"),
        "probe --json must not enter alt-screen"
    );
}

#[test]
fn probe_fallback_with_query_filters_listing() {
    // spec: TUI-2 - query arg applies in fallback (non-TUI) mode.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui", "review"]);
    assert!(
        r.success,
        "probe --no-tui query should succeed: {}",
        r.stderr
    );
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(!r.stdout.contains("agent:dev"), "filtered: {}", r.stdout);
}

#[test]
fn probe_fallback_with_kind_filter_narrows_listing() {
    // spec: TUI-2 - --kind arg applies in fallback mode.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui", "--kind", "skill"]);
    assert!(
        r.success,
        "probe --no-tui --kind should succeed: {}",
        r.stderr
    );
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(!r.stdout.contains("agent:dev"), "filtered: {}", r.stdout);
    assert!(!r.stdout.contains("rule:style"), "filtered: {}", r.stdout);
}

#[test]
fn probe_fallback_seed_query_with_no_tui() {
    // spec: TUI-2 - query args are seed state in both modes; with --no-tui the
    // query filters the listing (same as plain `probe <query>`).
    let sb = melded();
    let r1 = sb.mind(&["probe", "review"]);
    let r2 = sb.mind(&["probe", "--no-tui", "review"]);
    assert!(r1.success);
    assert!(r2.success);
    // Both produce the same result (same filter applied).
    assert_eq!(
        r1.stdout, r2.stdout,
        "--no-tui must not change filter behavior"
    );
}

#[test]
fn probe_fallback_with_source_filter_narrows_listing() {
    // spec: TUI-2 - the --source seed arg filters the listing in fallback mode,
    // matching plain `probe --source` (CLI-83). Only query and --kind were
    // previously exercised in fallback; this closes the --source axis.
    let sb = melded();
    let matched = sb.mind(&["probe", "--no-tui", "--source", "agents"]);
    assert!(
        matched.success,
        "probe --no-tui --source should succeed: {}",
        matched.stderr
    );
    assert!(
        matched.stdout.contains("skill:review"),
        "matching source listing: {}",
        matched.stdout
    );

    let unmatched = sb.mind(&["probe", "--no-tui", "--source", "nonesuch"]);
    assert!(
        unmatched.success,
        "probe --no-tui --source nonesuch should succeed: {}",
        unmatched.stderr
    );
    assert!(
        !unmatched.stdout.contains("skill:review"),
        "a non-matching --source must exclude items: {}",
        unmatched.stdout
    );
}

#[test]
fn probe_non_tty_returns_promptly_and_does_not_hang() {
    // spec: TUI-2 - a non-TTY `mind probe` (the harness pipes stdout) must fall
    // back to the listing and EXIT, never entering the interactive event loop
    // that blocks on terminal input. Regression guard: if the fallback branch
    // broke and the TUI launched here, the process would block on event::read
    // and this bounded wait would time out.
    use std::time::{Duration, Instant};

    let sb = melded();
    // Spawn directly so we can bound the wall-clock time. stdin is the inherited
    // null/closed handle of the test process (not a TTY), matching the non-TTY
    // condition; we do NOT feed any input, so a real TUI would hang.
    let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["probe"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mind probe");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(status.success(), "non-TTY probe should exit successfully");
                break;
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "non-TTY `mind probe` did not exit within 10s - it likely entered the TUI event loop instead of falling back"
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    let out = child.wait_with_output().expect("collect output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skill:review"),
        "fallback listing expected: {stdout}"
    );
    assert!(
        !stdout.contains("\x1b[?1049h"),
        "non-TTY probe must not enter the alt-screen"
    );
}
