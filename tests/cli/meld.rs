//! `meld` (spec/cli.md).

use super::*;

/// Find the line containing "run `mind ...`" (or "hint: run `mind ...`"),
/// pull the backtick-fenced command out of it, and return it tokenized (with
/// the leading `mind` dropped) ready to hand to [`Sandbox::mind`]. Panics if
/// no such line is found, so a round-trip test can never pass by quietly
/// finding nothing to run.
fn remedy_argv(text: &str) -> Vec<String> {
    let line = text
        .lines()
        .find(|l| l.contains("run `mind ") || l.contains("run\n`mind "))
        .unwrap_or_else(|| panic!("no `run \\`mind ...\\`` remedy line in: {text:?}"));
    let start = line.find("`mind ").expect("checked above") + 1;
    let rest = &line[start..];
    let end = rest.find('`').unwrap_or(rest.len());
    let cmd = &rest[..end];
    let mut argv = shell_split(cmd);
    assert_eq!(
        argv.first().map(String::as_str),
        Some("mind"),
        "the remedy must be a `mind` invocation: {cmd:?}"
    );
    argv.remove(0);
    argv
}

// ---------------------------------------------------------------------------
// CLI-225: printed-remedy shell-quoting at commands/ and install.rs sites
// (the error.rs remedies were already covered by HOOK-106's tests).
// ---------------------------------------------------------------------------

#[test]
fn remeld_ignored_flags_note_shell_quotes_a_metachar_alias() {
    // spec: CLI-225 -- the re-meld ignored-flags note's `mind unmeld <name>`
    // remedy embeds the source-influenced `--as` alias. `validate_prefix`
    // rejects only path traversal, not shell metacharacters, so a `'`/`;`/
    // `$(...)` alias must be shell-quoted rather than spliced raw. Proven as a
    // real round trip: the printed command, tokenized as a shell would, must
    // unmeld exactly the aliased instance.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let alias = "it's; touch pwned; echo $(whoami)";
    let first = sb.mind(&["meld", &spec, "--as", alias, "--register-only"]);
    assert!(
        first.success,
        "first meld failed: {} {}",
        first.stdout, first.stderr
    );

    let again = sb.mind(&[
        "meld",
        &spec,
        "--as",
        alias,
        "--register-only",
        "--add-root",
        ".claude",
    ]);
    assert!(
        again.success,
        "a re-meld is not an error: {} {}",
        again.stdout, again.stderr
    );

    // The registered identity is `local/<base>/<repo>@<alias>` (STO-58): the
    // alias is a suffix, not the whole name, so the quoted form covers the
    // whole identity string.
    let source_name = format!("local/{}/agents@{alias}", sb.base_name());
    let quoted = shell_quote(&source_name);
    assert!(
        again.stdout.contains(&format!("mind unmeld {quoted}")),
        "the remedy must shell-quote the source identity: {}",
        again.stdout
    );
    assert!(
        !again.stdout.contains(&format!("mind unmeld {source_name}")),
        "the raw identity must never be spliced in unquoted: {}",
        again.stdout
    );

    let argv = remedy_argv(&again.stdout);
    let mut args: Vec<&str> = argv.iter().map(String::as_str).collect();
    // A piped run confirms the removal with `--yes` (CLI-259).
    args.push("--yes");
    let r = sb.mind(&args);
    assert!(
        r.success,
        "the round-tripped remedy must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "the round-tripped `mind unmeld` must have removed exactly the aliased instance"
    );
}

#[test]
fn learn_not_found_hint_shell_quotes_a_metachar_query() {
    // spec: CLI-225, CLI-179 -- the `mind probe <query>` hint on a `learn`
    // ItemNotFound echoes the user's own argument verbatim; shell-quote it
    // before printing so the pasteable remedy cannot inject.
    let sb = melded();
    let query = "a'; touch pwned; echo $(whoami)";
    let r = sb.mind(&["learn", query]);
    assert!(
        !r.success,
        "learn must fail for an unknown item: {}",
        r.stdout
    );

    let quoted = shell_quote(query);
    assert!(
        r.stderr.contains(&format!("mind probe {quoted}")),
        "the hint must shell-quote the query: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains(&format!("mind probe {query}")),
        "the raw query must never be spliced in unquoted: {}",
        r.stderr
    );
}

#[test]
fn unmeld_unlink_only_remaining_items_hint_shell_quotes_a_metachar_alias() {
    // spec: CLI-225, CLI-22 -- `unmeld --unlink-only`'s "N item(s) remain
    // installed" note prints `mind forget '<source>#*'`; the source identity
    // must be shell-quoted, not framed in bare single quotes (a `'` in the
    // identity would otherwise break out of that frame). Proven as a real
    // round trip: the printed command actually forgets the leftover item.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let alias = "it's a source";
    assert!(sb.mind(&["meld", &spec, "--as", alias]).success);
    // `--as` selects the instance by suffix match (CLI-5), but the note prints
    // the full registered identity `local/<base>/<repo>@<alias>` (STO-58).
    let source_name = format!("local/{}/agents@{alias}", sb.base_name());
    // `--as` also sets the effective namespace prefix, so `review`'s effective
    // name is `<alias>:review`, not the bare name; `*` installs every item
    // from the one melded source regardless of its prefix.
    let learn_all = sb.mind(&["learn", "*"]);
    assert!(
        learn_all.success,
        "learn * failed: {} {}",
        learn_all.stdout, learn_all.stderr
    );

    let r = sb.mind(&["unmeld", &source_name, "--unlink-only", "--yes"]);
    assert!(r.success, "unmeld failed: {} {}", r.stdout, r.stderr);

    let quoted = shell_quote(&format!("{source_name}#*"));
    assert!(
        r.stdout.contains(&format!("mind forget {quoted}")),
        "the remedy must shell-quote the source identity: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains(&format!("mind forget '{source_name}#*'")),
        "the identity must never be framed in bare single quotes: {}",
        r.stdout
    );

    let argv = remedy_argv(&r.stdout);
    // The remedy names 3 items via a glob; append --yes so this round trip
    // can run non-interactively (unrelated to what CLI-225 proves: that the
    // glob/source identity itself parses back to exactly the intended items).
    let mut args: Vec<&str> = argv.iter().map(String::as_str).collect();
    args.push("--yes");
    let forget = sb.mind(&args);
    assert!(
        forget.success,
        "the round-tripped remedy must succeed: {} {}",
        forget.stdout, forget.stderr
    );
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "the round-tripped `mind forget` must have removed every item"
    );
}

#[test]
fn recall_not_installed_hint_shell_quotes_an_item_name_with_a_quote() {
    // spec: CLI-225 -- a re-meld's per-item listing (`source_status`, reached
    // via `meld <already-melded-source> --register-only`) prints `mind learn
    // '<kind:name>'` for each not-yet-installed item; the item's own name is
    // source content (only restricted against path separators/NUL,
    // `is_safe_item_name`), so a `'` in it must be shell-quoted before it
    // lands in the pasteable remedy.
    let sb = Sandbox::bare("quoteitems");
    sb.write_and_commit(
        "skills/rev'iew/SKILL.md",
        "---\ndescription: a quote-carrying item\n---\n# skill\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    // Re-meld the already-registered source to reach `source_status`'s
    // per-item listing.
    let r = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(r.success, "re-meld failed: {} {}", r.stdout, r.stderr);

    let quoted = shell_quote("skill:rev'iew");
    assert!(
        r.stdout.contains(&format!("mind learn {quoted}")),
        "the hint must shell-quote the item key: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("mind learn 'skill:rev'iew'"),
        "the raw item key must never be framed in bare single quotes: {}",
        r.stdout
    );
}

#[test]
fn meld_default_non_tty_install_note_shell_quotes_a_metachar_alias() {
    // spec: CLI-225 -- the "registered only, nothing installed (not a TTY)"
    // note prints `mind learn '<source>#*'`; same rule as the unlink-only
    // remaining-items hint, at the `install_source_items` call site. Proven
    // as a real round trip: the printed command actually installs the item.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let alias = "it's; touch pwned";
    let r = sb.mind(&["meld", &spec, "--as", alias]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("nothing installed"),
        "must note nothing was installed (not a TTY): {}",
        r.stdout
    );

    let source_name = format!("local/{}/agents@{alias}", sb.base_name());
    let quoted = shell_quote(&format!("{source_name}#*"));
    assert!(
        r.stdout.contains(&format!("mind learn {quoted}")),
        "the note must shell-quote the source identity: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains(&format!("mind learn '{source_name}#*'")),
        "the identity must never be framed in bare single quotes: {}",
        r.stdout
    );

    let argv = remedy_argv(&r.stdout);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let learn = sb.mind(&args);
    assert!(
        learn.success,
        "the round-tripped remedy must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        sb.mind(&["recall"]).stdout.contains("installed @"),
        "the round-tripped `mind learn` must have installed the item(s)"
    );
}

#[test]
fn curator_add_roots_warning_shell_quotes_a_metachar_nested_source_name() {
    // spec: CLI-225, DSC-60 -- the DSC-60 "apply add-roots yourself" hint
    // prints `mind meld <nested-source> --add-root <dir>`; the nested
    // source's identity is source-influenced (its local path's own basename)
    // and must be shell-quoted.
    let nested = Sandbox::bare("it's a nested source");
    nested.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: base skill\n---\n# base\n",
    );
    nested.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: add-root contributed skill\n---\n# addon\n",
    );
    nested.write_and_commit("mind.toml", "[source]\ndescription = \"onboarded\"\n");

    let registry = Sandbox::bare("addroot-registry-quote-test");
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

    // The nested source's identity is `local/<base>/<name>` (see `link_name`
    // in tests/cli_item_link.rs for the same pattern); locate it from the
    // warning text itself rather than reconstructing it, since only the
    // fixture's own temp-dir basename is not under our control.
    assert!(
        r.stderr.contains("mind meld") && r.stderr.contains("--add-root"),
        "the DSC-60 warning must offer the consumer-side add-root remedy: {}",
        r.stderr
    );
    // Whatever identity the remedy names, it must appear shell-quoted:
    // extract the `mind meld <id> --add-root` command and confirm the
    // fixture's raw quote-carrying basename is not spliced in bare.
    let start = r.stderr.find("`mind meld ").expect("remedy present") + 1;
    let rest = &r.stderr[start..];
    let end = rest.find('`').unwrap_or(rest.len());
    let cmd = &rest[..end];
    assert!(
        cmd.contains(r"'\''"),
        "the nested source's quote-carrying name must be shell-quoted \
         (the '\\'' idiom must appear) in: {cmd:?}"
    );
}

#[test]
fn meld_registers_source_and_lists_items() {
    // spec: CLI-10, CLI-72
    let sb = melded();
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.success);
    assert!(r.stdout.contains("agents"), "sources: {}", r.stdout);
}

#[test]
fn meld_with_zero_discovered_items_reports_guidance() {
    // spec: CLI-205 - B7: a top-level meld of a repo with no items discoverable
    // by convention scanning must say so explicitly, naming the convention
    // paths and the escapes, with a non-success glyph, rather than reading
    // identically to a legitimate empty source (`melded X (0 item(s))`).
    let sb = Sandbox::bare("zero-item-repo");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a zero-item meld is not a hard error: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("(0 item(s))"),
        "must still report the 0 item(s) count: {}",
        r.stdout
    );
    // ASCII fallback in this non-TTY harness (CLI-151): `!` is the warning
    // glyph, `+` is the success glyph (CLI-152). The zero-item line must use
    // the warning glyph, not the success one.
    assert!(
        r.stdout.contains("! melded"),
        "a zero-item meld must use the warning glyph, not the success one: {}",
        r.stdout
    );
    for hint in [
        "skills/<name>/SKILL.md",
        "agents/<name>.md",
        "rules/<name>.md",
        "tools/<name>/",
        "--root",
        "--add-root",
        "--flat-skills",
    ] {
        assert!(
            r.stdout.contains(hint),
            "guidance must mention `{hint}`: {}",
            r.stdout
        );
    }
}

#[test]
fn meld_with_add_root_discovering_items_suppresses_zero_item_note() {
    // spec: CLI-205 - the guidance from the previous test is exactly the fix
    // for the repro in the finding: the escape it names (`--add-root`) turns
    // the same zero-item repo into a normal, successful meld.
    let sb = Sandbox::bare("addroot-fixes-zero");
    sb.write_and_commit(
        ".claude/skills/foo/SKILL.md",
        "---\nname: foo\ndescription: foo\n---\n# foo\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--add-root", ".claude"]);
    assert!(
        r.success,
        "meld --add-root failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("(1 item(s))") && r.stdout.contains("+ melded"),
        "with --add-root discovering the item, the note must not fire: {}",
        r.stdout
    );
}

#[test]
fn remeld_notes_the_discovery_flags_it_ignored() {
    // spec: CLI-206 - a re-meld never re-clones or re-registers, so the
    // discovery flags cannot take effect. Silently dropping them is the worse
    // failure: the flag help reads as though they are settable later, so a
    // user changing them on an existing source otherwise gets no command that
    // works and no error saying so. `--pin` is deliberately excluded here: a
    // re-meld DOES honor it (CLI-209, see the `remeld_pin_*` tests below), so
    // it must not appear in this ignored-flags note.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let first = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(first.success, "first meld failed: {}", first.stderr);

    let again = sb.mind(&[
        "meld",
        &spec,
        "--register-only",
        "--add-root",
        ".claude",
        "--flat-skills",
    ]);
    assert!(
        again.success,
        "a re-meld is not an error: {} {}",
        again.stdout, again.stderr
    );
    assert!(
        again.stdout.contains("--add-root") && again.stdout.contains("--flat-skills"),
        "the note must name each ignored flag: {}",
        again.stdout
    );
    assert!(
        again.stdout.contains("ignored") && again.stdout.contains("unmeld"),
        "the note must say they were ignored and what to do instead: {}",
        again.stdout
    );
    assert!(
        !again.stdout.contains("--pin"),
        "the ignored-flags note must not name --pin now that a re-meld honors it (CLI-209): {}",
        again.stdout
    );

    // A re-meld carrying none of those flags stays quiet.
    let quiet = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(
        !quiet.stdout.contains("ignored"),
        "a re-meld with no discovery flags must print no note: {}",
        quiet.stdout
    );
}

#[test]
fn meld_yes_installs_all_source_items() {
    // spec: CLI-23 - `meld --yes` registers the source and installs all of its
    // items without prompting (so it works in this non-TTY harness too).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    let recall = sb.mind(&["recall"]);
    for item in ["review", "dev", "style"] {
        assert!(
            recall.stdout.contains(item),
            "{item} should be installed after `meld --yes`: {}",
            recall.stdout
        );
    }
}

#[test]
fn meld_link_only_registers_without_installing() {
    // spec: CLI-23 - `--link-only` stops at registering the source; nothing is
    // installed and there is no install prompt.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source must be registered"
    );
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "--link-only must not install any items"
    );
}

#[test]
fn meld_default_non_tty_registers_only_and_notes_install() {
    // spec: CLI-23 - a default `meld` over piped (non-TTY) stdin registers the
    // source but installs nothing, and prints how to install later.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source must be registered"
    );
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "a non-TTY default meld must not install items"
    );
    assert!(
        r.stdout.contains("learn") && r.stdout.contains("#*"),
        "it should note how to install later: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("nothing installed"),
        "the note must say explicitly that nothing was installed: {}",
        r.stdout
    );
}

#[test]
fn meld_uses_declared_prefix_when_installing() {
    // spec: CLI-24 - a non-interactive meld accepts a source's declared
    // `[source].prefix`; installed items are namespaced `<prefix>:<name>`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "meld of a prefixed source should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("jk:review"),
        "items must carry the declared prefix: {recall}"
    );
}

#[test]
fn meld_as_empty_overrides_a_declared_prefix() {
    // spec: CLI-24 - `--as ''` is the explicit "no prefix" override and
    // suppresses a declared `[source].prefix`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--as", "", "--yes"]).success,
        "meld --as '' should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("review"), "items must install: {recall}");
    assert!(
        !recall.contains("jk:"),
        "the declared prefix must be overridden to none: {recall}"
    );
}

#[test]
fn meld_namespace_empty_overrides_a_declared_prefix() {
    // spec: CLI-159 CLI-24 - `--namespace ''` (the renamed `--as`) is the explicit
    // "no prefix" override and suppresses a declared `[source].prefix`, identically
    // to the deprecated `--as ''`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "", "--yes"])
            .success,
        "meld --namespace '' should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("review"), "items must install: {recall}");
    assert!(
        !recall.contains("jk:"),
        "the declared prefix must be overridden to none: {recall}"
    );
}

#[test]
fn meld_with_no_arg_melds_the_current_directory() {
    // spec: CLI-25 - `mind meld` with no repo argument melds the directory it is
    // run in. `--link-only` keeps the test to just the registration.
    let sb = Sandbox::new();
    let r = sb.mind_cwd(&["meld", "--link-only"], &sb.source);
    assert!(
        r.success,
        "no-arg meld of the cwd failed: {} {}",
        r.stdout, r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("agents"),
        "the current directory must be registered as a source: {sources}"
    );
}

#[test]
fn local_source_is_read_from_its_working_tree() {
    // spec: CLI-27 - a linked local source is read from its working tree, so an
    // untracked mind.toml is seen; it is never cloned, and unmeld never deletes it.
    let sb = Sandbox::bare("worktree-src");
    // Commit an item, then add an UNTRACKED mind.toml (in no commit).
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ndescription = \"live working tree\"\n",
    );
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    // No clone is made under the sources tree; the working tree is the source.
    assert!(
        !clone_dir_of(&sb, "worktree-src").exists(),
        "a linked local source must not be cloned"
    );
    // The untracked mind.toml is read live from the working tree.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("live working tree"),
        "the untracked mind.toml must be read from the working tree: {sources}"
    );

    // unmeld leaves the working tree intact.
    assert!(
        sb.mind(&["unmeld", "worktree-src", "--unlink-only", "--yes"])
            .success
    );
    assert!(
        sb.source.join("skills/a/SKILL.md").exists(),
        "unmeld must not delete the linked working tree"
    );
}

// ---------------------------------------------------------------------------
// U40 / CLI-212 / CLI-213: a linked source whose working tree vanishes after
// meld must degrade the listing verbs, not take the whole command down.
// ---------------------------------------------------------------------------

#[test]
fn remeld_of_an_uninstalled_source_offers_to_install() {
    // spec: CLI-12 - re-melding is not an error; with items still uninstalled it
    // routes to the default install flow (here non-TTY, so it notes how to install).
    let sb = melded(); // non-TTY meld registers but does not install
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld should not error: {}", r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "re-meld must report the source is already melded: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("to install"),
        "with items uninstalled, re-meld must offer to install them: {}",
        r.stdout
    );
}

#[test]
fn remeld_of_an_installed_source_shows_item_status() {
    // spec: CLI-12 - when nothing remains to install, re-melding prints each
    // item's install state and the commit it was installed from.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "initial meld+install"
    );
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld should not error: {}", r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "re-meld must report the source is already melded: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:review") && r.stdout.contains("installed @"),
        "re-meld of a fully installed source must show item status with commits: {}",
        r.stdout
    );
}

#[test]
fn learn_pin_on_a_plain_ref_is_ignored_with_a_note() {
    // spec: CLI-200 - `--pin` applies only to a deep-link URL; for a plain item
    // ref it is a no-op and prints a note, but the install still proceeds.
    let sb = melded();
    let r = sb.mind(&["learn", "review", "--pin"]);
    assert!(
        r.success,
        "learn --pin on a plain ref should still install: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("--pin is ignored"),
        "expected the ignored-pin note on stderr, got: {}",
        r.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the skill must still be installed despite the ignored --pin"
    );
}

#[test]
fn meld_as_prefixes_names_links_and_refs() {
    // spec: CLI-13, NS-1, NS-2
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:jk:review"), "{}", probe.stdout);
    assert!(probe.stdout.contains("agent:jk:dev"), "{}", probe.stdout);
    // The bare names must not appear.
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);

    // Install under the prefixed name; symlink lands at the prefixed location.
    assert!(sb.mind(&["learn", "jk:review"]).success);
    let link = sb.claude_home.join("skills/jk:review");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("namespace:jk"),
        "{}",
        sources.stdout
    );
}

#[test]
fn meld_namespace_flag_sets_prefix_and_as_alias_still_works() {
    // spec: CLI-159 - `--namespace` (short `-n`) is the renamed `--as`; both set
    // the source's namespace. `--as` is retained as a hidden deprecated alias.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // --namespace sets the prefix; items install under jk:<bare>.
    assert!(sb.mind(&["meld", &spec, "--namespace", "jk"]).success);
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "--namespace must set prefix: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:review"),
        "bare name must not appear when prefixed: {}",
        probe.stdout
    );

    // Verify the short form -N also works (a separate sandbox to avoid
    // re-meld collision).
    // spec: CLI-163 - -N is the new short form for --namespace on meld.
    let sb2 = Sandbox::new();
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2, "-N", "zz"]).success);
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("skill:zz:review"),
        "-N short form must set prefix: {}",
        probe2.stdout
    );

    // Verify the hidden --as alias still works identically.
    let sb3 = Sandbox::new();
    let spec3 = sb3.source_spec();
    assert!(sb3.mind(&["meld", &spec3, "--as", "qq"]).success);
    let probe3 = sb3.mind(&["probe"]);
    assert!(
        probe3.stdout.contains("skill:qq:review"),
        "--as deprecated alias must still set prefix: {}",
        probe3.stdout
    );
}

#[test]
fn review_namespace_flag_evaluates_under_prefix() {
    // spec: CLI-159, CLI-163 - `review --namespace <prefix>` (short `-N`) evaluates
    // the source under that prospective prefix; `--as` is a hidden deprecated alias.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // --namespace accepted (same behavior as former --as).
    let r = sb.mind(&["review", &spec, "--namespace", "jk"]);
    assert!(
        r.success,
        "review --namespace must exit 0 for clean source: {} {}",
        r.stdout, r.stderr
    );

    // Short -N accepted (CLI-163: -n is reserved for --dry-run).
    let r2 = sb.mind(&["review", &spec, "-N", "jk"]);
    assert!(
        r2.success,
        "review -N must exit 0 for clean source: {} {}",
        r2.stdout, r2.stderr
    );
}

#[test]
fn meld_with_ssh_config_still_melds_a_local_source() {
    // spec: CLI-19 - `ssh = true` makes meld prefer SSH for https remotes, but a
    // local path is never rewritten, so a local-path meld still works and the
    // recorded URL stays the local path (no git@ rewrite).
    let sb = Sandbox::new();
    write(&sb.mind_home.join("config.toml"), "ssh = true\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "ssh-config meld of a local source should succeed: {}",
        r.stderr
    );
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains(&spec),
        "a local source URL must be unchanged under ssh=true: {json}"
    );
    assert!(
        !json.contains("git@"),
        "a local path must not be rewritten to git@: {json}"
    );
}

#[test]
fn meld_follow_branch_clones_named_branch_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-follow");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--follow-branch", "stable"]);
    assert!(r.success, "meld --follow-branch: {}", r.stderr);

    // The recorded commit is at stable (sha_v1), not main tip (sha_v2).
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "follow-branch=stable should record sha_v1");

    // The persisted pin has kind=follow-branch and value=stable.
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("follow-branch"),
        "pin kind should be follow-branch: {pin_json}"
    );
    assert!(
        pin_json.contains("stable"),
        "pin value should be stable: {pin_json}"
    );
}

#[test]
fn meld_pin_tag_clones_at_tag_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-tag");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(r.success, "meld --pin-tag: {}", r.stderr);

    // Should be at the tagged commit.
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "pin-tag=v1.0 should record sha_v1");

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
fn meld_pin_ref_clones_at_specific_commit_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-ref");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-ref", &sha_v1]);
    assert!(r.success, "meld --pin-ref: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "pin-ref should record sha_v1");

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
fn meld_default_branch_pin_is_at_main_tip() {
    // spec: CLI-17 (no flag -> default branch), STO-18 (DefaultBranch persisted)
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("pintest-default");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld default: {}", r.stderr);

    // Default branch (main) tip is sha_v2.
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v2, "default branch should be at main tip");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("default-branch"),
        "pin kind should be default-branch: {pin_json}"
    );
}

#[test]
fn meld_two_pin_flags_is_conflicting_pin_error() {
    // spec: CLI-17 (at most one pin flag)
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Two flags at once is an error.
    let r = sb.mind(&["meld", &spec, "--follow-branch", "main", "--pin-tag", "v1"]);
    assert!(
        !r.success,
        "two pin flags must be rejected: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // CLI-17 names the structured `ConflictingPin` error, so the flags are kept
    // independent at the clap layer and this is what surfaces (not a clap usage
    // string). The exit is non-zero and nothing is registered.
    assert!(
        r.stderr.contains("conflicting pin flags"),
        "expected the structured ConflictingPin error, got stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "nothing should be registered after a conflict error: {}",
        sources.stdout
    );
}

// ---- reworked pin CLI: single `--pin` verb (CLI-200, CLI-201, CLI-202) -------

#[test]
fn meld_pin_head_freezes_default_tip() {
    // spec: CLI-200, CLI-18 - `--pin HEAD` freezes the resolved floating tip
    // (the default branch here) to its current commit as an immutable ref.
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("pintest-pin-head");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "HEAD"]);
    assert!(r.success, "meld --pin HEAD: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v2, "--pin HEAD should freeze the main tip");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "--pin HEAD should persist a ref pin: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v2),
        "the frozen ref should be the tip sha: {pin_json}"
    );
}

#[test]
fn meld_pin_requires_a_value() {
    // spec: CLI-200 - `--pin` takes a required value; a valueless `--pin` is a
    // clap usage error. Requiring a value is what removes the optional-value
    // positional-swallow footgun (there is no bare form to consume the next arg).
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-noval");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin"]);
    assert!(!r.success, "valueless --pin must be a usage error");
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "nothing registered after a valueless --pin: {}",
        sources.stdout
    );
}

#[test]
fn meld_pin_empty_branch_name_is_invalid_ref() {
    // spec: CLI-201 - `--pin branch=` (empty name) reaches validate_ref_value and
    // is rejected, not silently taken as a follow-branch of "".
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-emptybranch");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "branch="]);
    assert!(!r.success, "an empty branch name must fail");
    assert_eq!(
        source_count(&sb),
        0,
        "nothing registered after an empty branch name"
    );
}

#[test]
fn meld_pin_abbreviated_sha_freezes_that_commit() {
    // spec: CLI-200 - a bare ref that is an abbreviated sha resolves and freezes
    // to the full commit, same as a 40-hex sha.
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-shortsha");
    let spec = sb.source_spec();

    let short = &sha_v1[..8];
    let r = sb.mind(&["meld", &spec, "--pin", short]);
    assert!(r.success, "meld --pin <short-sha>: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_v1,
        "short sha should resolve to the full commit"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\"") && pin_json.contains(&sha_v1),
        "a short-sha freeze should persist the full-sha ref: {pin_json}"
    );
}

#[test]
fn meld_pin_branch_value_freezes_that_branch_tip() {
    // spec: CLI-200 - `--pin <branch>` resolves the branch to its current commit
    // and freezes it (a snapshot, not a follow).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-branch");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "stable"]);
    assert!(r.success, "meld --pin stable: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "--pin stable should freeze stable's tip");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "--pin <branch> should persist a ref, not follow-branch: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v1),
        "the frozen ref should be stable's sha: {pin_json}"
    );
}

#[test]
fn meld_pin_tag_value_freezes_tag_commit() {
    // spec: CLI-200 - `--pin <tag>` resolves the tag to its current commit and
    // persists it as a ref (the resolved sha, NOT the literal tag name, so a later
    // upstream re-point of the tag does not move this source).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-tagval");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "v1.0"]);
    assert!(r.success, "meld --pin v1.0: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "--pin v1.0 should freeze the tag's commit");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "--pin <tag> should persist a ref, not a tag pin: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v1) && !pin_json.contains("v1.0"),
        "the ref value must be the resolved sha, not the literal tag: {pin_json}"
    );
}

#[test]
fn meld_pin_sha_value_is_exact_ref() {
    // spec: CLI-200 - `--pin <sha>` freezes that exact commit.
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-sha");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", &sha_v1]);
    assert!(r.success, "meld --pin <sha>: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "--pin <sha> should record that sha");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\"") && pin_json.contains(&sha_v1),
        "--pin <sha> should persist a ref at that sha: {pin_json}"
    );
}

#[test]
fn meld_pin_head_freezes_the_directive_selected_branch() {
    // spec: CLI-200 - `--pin HEAD` freezes the point the lower-precedence layers
    // chose (here a `[source]` follow-branch directive), not the default branch:
    // it changes float->freeze, it does not change the branch selection.
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-directive");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "HEAD"]);
    assert!(r.success, "meld --pin HEAD over directive: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_v1,
        "--pin HEAD should freeze stable's tip (the directive's branch)"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "the frozen directive branch should persist as a ref: {pin_json}"
    );
}

#[test]
fn meld_pin_branch_equals_tracks_branch() {
    // spec: CLI-201 - `--pin branch=<name>` tracks that branch (float).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-follow-branch-spec");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "branch=stable"]);
    assert!(r.success, "meld --pin branch=stable: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "--pin branch=stable records stable's tip");
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("follow-branch") && pin_json.contains("stable"),
        "pin should be follow-branch=stable: {pin_json}"
    );
}

#[test]
fn meld_pin_tag_equals_tracks_moving_tag() {
    // spec: CLI-201 - `--pin tag=<name>` records a tag pin (moving tag).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-follow-tag-spec");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "tag=v1.0"]);
    assert!(r.success, "meld --pin tag=v1.0: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "--pin tag=v1.0 records the tag's commit");
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\"") && pin_json.contains("v1.0"),
        "pin should be tag=v1.0: {pin_json}"
    );
}

#[test]
fn meld_pin_bad_key_is_usage_error() {
    // spec: CLI-201 - a value with an unrecognized `key=` form is a BadPinSpec
    // error, so a mistyped key is not silently treated as a ref name.
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-badkey");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "brnach=stable"]);
    assert!(!r.success, "an unrecognized --pin key must fail");
    assert!(
        r.stderr.contains("invalid --pin value"),
        "expected the BadPinSpec error, got: {}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "nothing registered after a bad --pin value: {}",
        sources.stdout
    );
}

#[test]
fn meld_pin_frozen_branch_does_not_advance_on_sync() {
    // spec: CLI-200, CLI-55 - a `--pin <branch>` freeze records a ref, so a later
    // upstream commit on that branch does NOT move the source on sync (the
    // freeze-vs-follow distinction, proven behaviorally at the meld level).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-frozen-sync");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin", "stable"]);
    assert!(r.success, "meld --pin stable: {}", r.stderr);
    assert_eq!(read_source_commit(&sb), sha_v1, "frozen at stable's tip");

    // Advance the stable branch upstream, then sync.
    git(&sb.source, &["checkout", "-q", "stable"]);
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v3\n---\n# dev v3\n",
    );
    git(&sb.source, &["commit", "-aqm", "stable moves"]);
    git(&sb.source, &["checkout", "-q", "main"]);

    assert!(sb.mind(&["sync"]).success, "sync should succeed");
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "a frozen ref must not advance when its branch moves"
    );
}

#[test]
fn meld_pin_freeze_satisfies_require_pinned_policy() {
    // spec: CLI-200, POL-20 - a `--pin <branch>` freeze persists a ref, so it is
    // accepted under a `require-pinned` policy even though the point it freezes is
    // an otherwise-floating branch (which a bare follow would be refused for).
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-pin-policy");
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pintest-pin-policy\"]\n",
    );
    let spec = sb.source_spec();

    // A plain follow of a branch is refused under require-pinned...
    let refused = sb.mind_env(
        &["meld", &spec, "--pin", "branch=stable"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !refused.success,
        "following a branch must be refused under require-pinned: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains("must be pinned"),
        "refusal should be the require-pinned one, got: {}",
        refused.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on refusal");

    // ...but freezing that same branch is accepted (it persists a ref).
    let r = sb.mind_env(
        &["meld", &spec, "--pin", "stable"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "a --pin freeze must satisfy require-pinned: {}",
        r.stderr
    );
    assert_eq!(read_source_commit(&sb), sha_v1);
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "the accepted pin must be a ref: {pin_json}"
    );
}

// --- CLI-209: a re-meld carrying `--pin` re-pins the already-registered
// source, unlike the CLI-206 discovery flags (which stay no-ops). ---------

#[test]
fn remeld_pin_repins_to_a_tag_and_changes_recorded_commit() {
    // spec: CLI-209 - a re-meld's `--pin` resolves the new pin, re-checks-out
    // the clone (here the resolved commit differs from the recorded one --
    // this is also the "first pin on a still-linked local source" transition,
    // since `make_pinnable_repo`'s first meld below carries no --pin), and
    // records the new pin and commit, reporting the change.
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("pintest-remeld-tag");
    let spec = sb.source_spec();

    let first = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(first.success, "first meld failed: {}", first.stderr);
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "an unpinned meld records the default branch tip"
    );

    let r = sb.mind(&["meld", &spec, "--register-only", "--pin", "v1.0"]);
    assert!(
        r.success,
        "re-meld --pin v1.0 failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("re-pinned"),
        "the source must report the re-pin: {}",
        r.stdout
    );
    // `--pin` must not ALSO be named on the CLI-206 ignored-flags note: it is
    // honored now (CLI-209), not dropped.
    assert!(
        !r.stdout.contains("--pin"),
        "--pin must not appear in an ignored-flags note now that it is honored: {}",
        r.stdout
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "the re-pin must record the tag's commit, not the prior default-branch tip"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\"") && pin_json.contains(&sha_v1),
        "a --pin <ref> freeze must persist a ref pin at the resolved commit: {pin_json}"
    );
}

#[test]
fn remeld_pin_to_the_same_value_is_a_noop() {
    // spec: CLI-209 - re-pinning to the value already recorded (no upstream
    // movement) is a no-op: the recorded pin/commit are unchanged and the
    // source reports that it is already pinned there rather than "re-pinned".
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-remeld-noop");
    let spec = sb.source_spec();

    let first = sb.mind(&["meld", &spec, "--register-only", "--pin", "v1.0"]);
    assert!(first.success, "first meld failed: {}", first.stderr);
    assert_eq!(read_source_commit(&sb), sha_v1);

    let r = sb.mind(&["meld", &spec, "--register-only", "--pin", "v1.0"]);
    assert!(
        r.success,
        "re-pin to the same tag failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("already pinned"),
        "re-pinning to the already-recorded value must say so: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("re-pinned"),
        "re-pinning to the already-recorded value must not report a change: {}",
        r.stdout
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "a no-op re-pin must not change the recorded commit"
    );
}

#[test]
fn remeld_pin_bad_value_fails_without_corrupting_source() {
    // spec: CLI-209 - a re-pin resolution/checkout failure (a nonexistent ref)
    // must leave the source's recorded pin/commit exactly as they were: every
    // git operation runs before any registry field is mutated or the registry
    // is saved.
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("pintest-remeld-bad-pin");
    let spec = sb.source_spec();

    let first = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(first.success, "first meld failed: {}", first.stderr);
    assert_eq!(read_source_commit(&sb), sha_v2);
    let pin_before = read_source_pin_json(&sb);

    let r = sb.mind(&[
        "meld",
        &spec,
        "--register-only",
        "--pin",
        "does-not-exist-anywhere",
    ]);
    assert!(
        !r.success,
        "a re-pin to a nonexistent ref must fail: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "a failed re-pin must not change the recorded commit"
    );
    assert_eq!(
        read_source_pin_json(&sb),
        pin_before,
        "a failed re-pin must not change the recorded pin"
    );
    assert_eq!(source_count(&sb), 1, "the source must remain registered");
}

#[test]
fn remeld_pin_marks_installed_items_out_of_date_when_commit_moves() {
    // spec: CLI-209 - a re-meld's `--pin` re-checks-out the source's clone at
    // the resolved commit; `recall`/`upgrade` already detect drift by
    // comparing an installed item's recorded hash against the source's
    // current content (CLI-75/LIFE-11), so once the pin moves the commit
    // forward, an item whose content changed there is reported out of date
    // exactly as any other upstream change is -- no separate mechanism is
    // needed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Fresh install: not out of date yet.
    let fresh = sb.mind(&["recall", "skill:review"]);
    assert!(
        !fresh.stdout.contains("out of date"),
        "a fresh install must not be out of date: {}",
        fresh.stdout
    );

    // Advance the source with a real commit (the item installed from the
    // prior commit is now behind).
    sb.edit_source();

    // Re-meld with `--pin HEAD`: freezes the (now-advanced) default branch
    // tip.
    let r = sb.mind(&["meld", &spec, "--pin", "HEAD"]);
    assert!(
        r.success,
        "re-meld --pin HEAD failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("re-pinned"),
        "the source must report a re-pin (the commit moved): {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]);
    assert!(
        after.success,
        "recall failed: {} {}",
        after.stdout, after.stderr
    );
    assert!(
        after.stdout.contains("out of date"),
        "the installed item must be reported out of date after the pin moved \
         the source's recorded commit: {}",
        after.stdout
    );
}

#[test]
fn meld_deprecated_aliases_map_and_conflict() {
    // spec: CLI-202 - the deprecated `--pin-ref` alias still freezes a commit,
    // and mixing a deprecated alias with `--pin` is still ConflictingPin.
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-deprecated-alias");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-ref", &sha_v1]);
    assert!(r.success, "deprecated --pin-ref: {}", r.stderr);
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\"") && pin_json.contains(&sha_v1),
        "--pin-ref should still persist a ref: {pin_json}"
    );

    // A deprecated alias plus `--pin` counts toward the at-most-one rule.
    let (sb2, _s1, _s2) = make_pinnable_repo("pintest-deprecated-conflict");
    let spec2 = sb2.source_spec();
    let r2 = sb2.mind(&["meld", &spec2, "--pin", "HEAD", "--follow-branch", "main"]);
    assert!(!r2.success, "--pin with a deprecated alias must conflict");
    assert!(
        r2.stderr.contains("conflicting pin flags"),
        "expected ConflictingPin, got: {}",
        r2.stderr
    );
}

#[test]
fn meld_pin_ref_unresolvable_is_git_error_and_registers_nothing() {
    // spec: CLI-18 - a pin that does not resolve in the remote is a `Git` error
    // and nothing is registered. The two-step clone re-clones at the resolved
    // pin after reading mind.toml; a failure of that second clone must not leave
    // a registered source nor a stray clone dir on disk.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-bad-ref");
    let spec = sb.source_spec();

    // A 40-char hex sha that does not exist in the remote.
    let bogus = "0123456789abcdef0123456789abcdef01234567";
    let r = sb.mind(&["meld", &spec, "--pin-ref", bogus]);
    assert!(
        !r.success,
        "unresolvable --pin-ref must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // It is a structured Git error (the checkout against the bogus sha fails).
    assert!(
        r.stderr.contains("git"),
        "expected a git error, got stderr={}",
        r.stderr
    );

    // Nothing registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after an unresolvable pin: {}",
        sources.stdout
    );
    // sources.json, if present, must not list the source.
    let sources_json = sb.mind_home.join("sources.json");
    if sources_json.exists() {
        let json = std::fs::read_to_string(&sources_json).unwrap();
        assert!(
            !json.contains("pintest-bad-ref"),
            "sources.json must not contain the failed source: {json}"
        );
    }
    // No stray clone dir is left under MIND_HOME for this source.
    let clone = clone_dir_of(&sb, "pintest-bad-ref");
    assert!(
        !clone.exists(),
        "an unresolvable pin must not leave a stray clone dir at {}",
        clone.display()
    );
}

#[test]
fn meld_pin_tag_unresolvable_is_git_error_and_registers_nothing() {
    // spec: CLI-18 - same as above for a tag that does not exist in the remote.
    // Here the re-clone uses `clone --branch <tag>` which fails outright, so the
    // staging clone dir never materializes.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-bad-tag");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-tag", "v9.9-does-not-exist"]);
    assert!(
        !r.success,
        "unresolvable --pin-tag must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("git"),
        "expected a git error, got stderr={}",
        r.stderr
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after an unresolvable tag pin: {}",
        sources.stdout
    );
    let clone = clone_dir_of(&sb, "pintest-bad-tag");
    assert!(
        !clone.exists(),
        "an unresolvable tag pin must not leave a stray clone dir at {}",
        clone.display()
    );
}

#[test]
fn pinned_local_meld_hook_failure_leaves_no_orphan_clone() {
    // spec: CLI-18, CLI-27, HOOK-30
    // A pinned local source (`--pin-ref`) is snapshotted into the sources tree
    // rather than read from the working tree. When a hook fails during that meld,
    // the snapshot clone must be removed (no orphan) and the source must not be
    // registered. The working tree itself must be untouched (CLI-27).
    let sb = sandbox_with_declared_hook("agents", "exit 1");
    let spec = sb.source_spec();

    // Read HEAD sha to supply as --pin-ref (so this becomes a pinned-local meld).
    let sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let r = sb.mind(&[
        "meld",
        &spec,
        "--pin-ref",
        &sha,
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "hook failure must fail the meld: {} {}",
        r.stdout, r.stderr
    );

    // Nothing registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "source must not be registered after a failed hook"
    );

    // The snapshot clone must be gone -- no orphan under the sources tree.
    let sources_tree = sb.mind_home.join("sources");
    if sources_tree.exists() {
        let clone = sources_tree
            .join("local")
            .join(sb.base_name())
            .join("agents");
        assert!(
            !clone.exists(),
            "pinned-local clone must be removed on hook failure, found orphan at {}",
            clone.display()
        );
    }

    // The working tree itself must be untouched (CLI-27).
    assert!(
        sb.source.exists(),
        "working tree must survive a failed pinned-local meld: {}",
        sb.source.display()
    );
}

/// `meld --yes --json` must emit exactly ONE top-level JSON object whose
/// `installed` array lists the items that were installed in this call.
/// Multiple JSON documents would break `json.loads`; silent installs would
/// make the meld result indistinguishable from a link-only run.
#[test]
fn meld_json_with_yes_installs_and_emits_single_object() {
    // spec: CLI-156 CLI-153
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["--json", "meld", &spec, "--yes"]);
    assert!(
        r.success,
        "meld --yes --json must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // stdout must parse as exactly one JSON object (no concatenated documents).
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "meld --yes --json stdout must be one valid JSON object, got error {e}: '{}'",
            r.stdout
        )
    });
    assert_eq!(v["action"], "meld", "action must be 'meld': {v}");
    assert_eq!(v["outcome"], "melded", "outcome must be 'melded': {v}");
    // The fixture has skill:review, agent:dev, rule:style => at least one installed key.
    let installed = v["installed"]
        .as_array()
        .expect("installed must be an array");
    assert!(
        !installed.is_empty(),
        "installed must not be empty when --yes is given: {v}"
    );
}

/// `meld --json` (no `--yes`) on a non-TTY (piped stdin) must not block
/// waiting for a confirmation prompt and must emit exactly ONE JSON object
/// with `pending_items >= 1`.
#[test]
fn meld_json_no_yes_non_tty_emits_single_object_with_pending() {
    // spec: CLI-156
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Passing empty stdin ensures this is non-TTY; no --yes so items are pending.
    let r = sb.mind_with_input(&["--json", "meld", &spec], Some(""));
    assert!(
        r.success,
        "meld --json (no --yes) must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "meld --json stdout must be one valid JSON object, got error {e}: '{}'",
            r.stdout
        )
    });
    assert_eq!(v["action"], "meld", "action must be 'meld': {v}");
    assert_eq!(v["outcome"], "melded", "outcome must be 'melded': {v}");
    let pending = v["pending_items"].as_u64().unwrap_or(0);
    assert!(
        pending >= 1,
        "pending_items must be >= 1 when items exist but --yes was not given: {v}"
    );
}

// ---- re-learn noop (CLI-157) -------------------------------------------------

fn fake_git_proxy_bin_dir(dir: &Path) -> PathBuf {
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-proxy-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  for a; do\n    case \"$a\" in\n      https://*)\n        echo \"fatal: unable to access '$a': Received HTTP code 407 from proxy after CONNECT\" >&2\n        exit 128\n        ;;\n    esac\n  done\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn meld_top_level_auth_failure_prints_ssh_hint() {
    // spec: CLI-177 -- auth failure on a direct meld emits SSH/credential hints.
    let sb = Sandbox::bare("auth-hint-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/private-repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail on auth error: {}", r.stderr);
    assert!(
        r.stderr.contains("git@example.com:owner/private-repo"),
        "auth hint must include the SSH URL: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("ssh = true"),
        "auth hint must mention `ssh = true` config option: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("credential helper"),
        "auth hint must mention credential helper: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_proxy_failure_prints_proxy_hint() {
    // spec: CLI-178 -- 407 proxy error on a direct meld emits HTTPS_PROXY hint.
    let sb = Sandbox::bare("proxy-hint-test");
    let fake_dir = fake_git_proxy_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail on proxy error: {}", r.stderr);
    assert!(
        r.stderr.contains("HTTPS_PROXY"),
        "proxy hint must mention HTTPS_PROXY: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("http.proxy"),
        "proxy hint must mention http.proxy: {}",
        r.stderr
    );
    // Auth hint must NOT be shown for a proxy failure.
    assert!(
        !r.stderr.contains("credential helper"),
        "auth hint must not appear for a proxy failure: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_clone_error_leads_with_git_stderr() {
    // spec: CLI-180 -- git's stderr leads the output; the explicitly constructed
    // "  command:" and "  store:" lines are suppressed without --verbose.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Clone a valid local repo but request a branch that does not exist; this
    // exercises the re-clone-at-pin error path (top_level=true).
    let r = sb.mind(&["meld", &spec, "--follow-branch", "nonexistent-branch"]);
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // git's stderr (the fatal error) must appear.
    assert!(
        r.stderr.contains("fatal:")
            || r.stderr.contains("error:")
            || r.stderr.contains("not found"),
        "git stderr must appear in output: {}",
        r.stderr
    );
    // Without --verbose the explicitly constructed "  command:" and "  store:"
    // lines must NOT appear (they expose the internal store path).
    assert!(
        !r.stderr.contains("  command: git clone"),
        "explicit command line must be absent without --verbose: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("  store:   "),
        "explicit store path line must be absent without --verbose: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_clone_error_verbose_shows_command_and_store() {
    // spec: CLI-180 -- under --verbose the command line and store path lines appear.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        "--verbose",
        &spec,
        "--follow-branch",
        "nonexistent-branch",
    ]);
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // Under --verbose the explicitly constructed lines must appear.
    assert!(
        r.stderr.contains("  command: git clone"),
        "explicit command line must appear under --verbose: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("  store:   "),
        "explicit store path line must appear under --verbose: {}",
        r.stderr
    );
}

#[test]
fn meld_json_clone_error_message_contains_git_cause() {
    // spec: CLI-184 -- under --json the JSON envelope's `message` field contains
    // the actual git stderr (the real cause) rather than the literal `<no stderr>`.
    let sb = Sandbox::bare("json-git-cause-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["--json", "meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stdout);
    // In --json mode the JSON envelope is on stdout; stderr should be empty.
    assert!(
        r.stderr.is_empty(),
        "no stderr output expected in --json mode: {}",
        r.stderr
    );
    let v = parse_json(&r.stdout);
    let msg = v["error"]["message"]
        .as_str()
        .expect("error.message must be a string");
    // The message must include the git failure text (the fake git says "Authentication failed").
    assert!(
        msg.contains("Authentication failed") || msg.contains("fatal:"),
        "message must contain the git error cause, not a placeholder: {msg}"
    );
    // The placeholder must NOT appear.
    assert!(
        !msg.contains("<no stderr>"),
        "message must not contain `<no stderr>` placeholder: {msg}"
    );
}

#[test]
fn meld_human_clone_error_no_no_stderr_literal() {
    // spec: CLI-185 -- in human mode, after git stderr is printed to the
    // terminal, the error trailer shows "(git output above)" not `<no stderr>`.
    let sb = Sandbox::bare("human-no-stderr-literal-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stderr);
    assert!(
        !r.stderr.contains("<no stderr>"),
        "the literal `<no stderr>` must not appear in human output: {}",
        r.stderr
    );
    // Git's actual error text must appear (printed directly before the hint).
    assert!(
        r.stderr.contains("Authentication failed") || r.stderr.contains("fatal:"),
        "git's stderr must appear in human output: {}",
        r.stderr
    );
}

fn fake_git_ansi_bin_dir(dir: &std::path::Path) -> std::path::PathBuf {
    // Creates a fake git that emits ANSI escape codes and Unicode bidi-override
    // characters in stderr on clone, to test that strip_ansi sanitizes them.
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-ansi-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // The ANSI sequence \033[31m is "red"; \xe2\x80\xae is a bidi-override char (U+202E).
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  printf 'fatal: \\033[31mANSI\\033[0m injection \\xe2\\x80\\xae bidi\\n' >&2\n  exit 128\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn meld_clone_error_strips_ansi_from_git_stderr() {
    // spec: CLI-186 -- ANSI escape codes in git stderr are stripped before
    // being printed to prevent terminal spoofing by a hostile remote.
    let sb = Sandbox::bare("ansi-strip-test");
    let fake_dir = fake_git_ansi_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // The text must appear but stripped of ANSI escapes.
    assert!(
        r.stderr.contains("ANSI"),
        "the text content must still appear: {}",
        r.stderr
    );
    assert!(
        !has_ansi_escape(&r.stderr),
        "no ANSI escape sequence must appear in the printed stderr: {:?}",
        r.stderr
    );
}

fn fake_git_ansi_fetch_bin_dir(dir: &std::path::Path) -> std::path::PathBuf {
    // A fake git that fails on `fetch` with ANSI escape + bidi-override bytes in
    // stderr, passing every other subcommand (clone, rev-parse, ...) through to
    // real git. This lets a source be melded (clone) with real git and then fail
    // its per-source `sync` fetch with hostile output, exercising the CLI-186
    // strip_ansi on the sync path (a different code path from the meld top-level
    // clone path already covered above).
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-ansi-fetch-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // \033[31m ... \033[0m is an ANSI color pair; \xe2\x80\xae is U+202E (bidi).
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"fetch\" ]; then\n  printf 'fatal: \\033[31mANSIFETCH\\033[0m injection \\xe2\\x80\\xae bidi\\n' >&2\n  exit 128\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn sync_per_source_error_strips_ansi_from_git_stderr() {
    // spec: CLI-186 -- the per-source `sync` failure path also sanitizes git
    // stderr via strip_ansi. The dev flagged this path had no dedicated ANSI
    // test (only the top-level meld clone path was covered). Drive it by melding
    // a tag-pinned local source (a real clone) with real git, then running sync
    // with a fake git whose `fetch` fails with ANSI/bidi bytes.
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("sync-ansi-strip-test");
    let spec = sb.source_spec();

    // Meld at a tag so the source is a clone (not a linked working tree): its
    // sync goes through git::sync_to_pin -> `git fetch`, which the fake fails.
    let melded = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(melded.success, "meld --pin-tag v1.0: {}", melded.stderr);

    let fake_dir = fake_git_ansi_fetch_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(&["sync"], &[("PATH", &new_path)]);
    // sync exits non-zero because the one source failed to fetch.
    assert!(
        !r.success,
        "sync must fail when the source's fetch fails: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The sanitized error text must still carry the git message content.
    assert!(
        r.stderr.contains("ANSIFETCH"),
        "the git error text content must still appear: {}",
        r.stderr
    );
    // But every ANSI escape must have been stripped before printing.
    assert!(
        !has_ansi_escape(&r.stderr),
        "no ANSI escape sequence may appear in the printed sync stderr: {:?}",
        r.stderr
    );
    // The bidi-override control char (U+202E) is an ANSI/control byte the
    // strip_ansi filter also removes; assert it did not survive.
    assert!(
        !r.stderr.contains('\u{202e}'),
        "the U+202E bidi-override char must not survive into printed stderr: {:?}",
        r.stderr
    );
}

#[test]
fn no_sources_melded_phrasing_sync() {
    // spec: CLI-187 -- `sync` with no sources uses the standardized phrase
    // "no sources melded; run `mind meld <owner/repo>` to add one".
    let sb = Sandbox::bare("cli187-sync-test");
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync with no sources must succeed: {}", r.stderr);
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "sync empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_recall_sources() {
    // spec: CLI-187 -- `recall --sources` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-recall-sources-test");
    let r = sb.mind(&["recall", "--sources"]);
    assert!(
        r.success,
        "recall --sources with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "recall --sources empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_recall() {
    // spec: CLI-187 -- `recall` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-recall-test");
    let r = sb.mind(&["recall"]);
    assert!(
        r.success,
        "recall with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "recall empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_probe() {
    // spec: CLI-187 -- `probe` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-probe-test");
    let r = sb.mind(&["probe", "anything"]);
    assert!(
        r.success,
        "probe with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "probe empty message must match the standard phrasing: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// meld --add-root: compose extra convention roots with the authoritative
// discovery layer (DSC-84..86, MKT-17, STO-55, CLI-197)
// ---------------------------------------------------------------------------

#[test]
fn re_pin_to_older_sha_then_upgrade_installs_older_content() {
    // spec: CLI-209 -- a re-meld with --pin re-pins (and re-checks out) the source.
    // spec: LIFE-11 -- an item is pending on ANY content change, so applying the
    // older content is an ordinary upgrade. Together these are the documented
    // downgrade/rollback recipe (docs/src/troubleshooting.md).
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("downgrade");
    let spec = sb.source_spec();

    // Meld pinned at v2 and install the agent: the store holds "dev v2".
    assert!(
        sb.mind(&["meld", &spec, "--pin", &sha_v2]).success,
        "meld --pin v2 must succeed"
    );
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");
    let link = sb.claude_home.join("agents/dev.md");
    let at_v2 = std::fs::read_to_string(&link).expect("installed agent");
    assert!(
        at_v2.contains("dev v2"),
        "the installed agent must start at v2: {at_v2}"
    );
    let store_path = sb.mind_home.join("store/agent/dev");
    let store_at_v2 = std::fs::read_to_string(&store_path).expect("store copy at v2");
    assert!(store_at_v2.contains("dev v2"), "{store_at_v2}");
    let manifest_v2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let hash_v2 = manifest_v2["items"]["agent:dev"]["hash"].clone();
    assert!(hash_v2.is_string(), "sanity: hash recorded at v2");

    // Downgrade: re-pin to the older commit, then upgrade applies the older
    // content (LIFE-11 pends on the hash change back to v1).
    assert!(
        sb.mind(&["meld", &spec, "--pin", &sha_v1]).success,
        "re-pin to v1 must succeed"
    );
    let up = sb.mind(&["upgrade", "dev", "--yes"]);
    assert!(up.success, "downgrade upgrade failed: {}", up.stderr);

    let at_v1 = std::fs::read_to_string(&link).expect("installed agent after downgrade");
    assert!(
        at_v1.contains("dev v1"),
        "the agent must be rolled back to v1: {at_v1}"
    );
    assert!(
        !at_v1.contains("dev v2"),
        "the v2 content must be gone after the downgrade: {at_v1}"
    );

    // The recipe must update the STORE copy and the manifest's recorded hash,
    // not just the lobe symlink target (a link-only downgrade would leave a
    // stale store copy and hash behind, which `recall`/`introspect` would then
    // misreport as drift).
    let store_at_v1 = std::fs::read_to_string(&store_path).expect("store copy after downgrade");
    assert!(
        store_at_v1.contains("dev v1") && !store_at_v1.contains("dev v2"),
        "the store copy must roll back to v1, not just the link: {store_at_v1}"
    );
    let manifest_v1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let hash_v1 = manifest_v1["items"]["agent:dev"]["hash"].clone();
    assert_ne!(
        hash_v1, hash_v2,
        "the manifest's recorded hash must advance to the v1 content's hash, \
         not stay pinned at v2's: {manifest_v1:#}"
    );
}

// ---------------------------------------------------------------------------
// forget --dropped (CLI-245)
// ---------------------------------------------------------------------------
