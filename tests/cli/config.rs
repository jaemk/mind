//! `config`, `completions`, and `man` (spec/cli.md).

use super::*;

#[test]
fn config_lobes_add_list_remove() {
    // spec: CLI-111, CLI-112, CLI-113
    let sb = Sandbox::new();
    let home_a = sb.base.join("lobeA");
    let home_b = sb.base.join("lobeB");
    let (a, b) = (home_a.display().to_string(), home_b.display().to_string());

    assert!(sb.mind(&["config", "lobes", "add", &a]).success);
    assert!(sb.mind(&["config", "lobes", "add", &b]).success);

    let list = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(list.contains(&a), "{list}");
    assert!(list.contains(&b), "{list}");

    // Configured lobes drive where learn links.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());

    // Remove one; it drops from the list, removing a missing one errors.
    assert!(sb.mind(&["config", "lobes", "remove", &a]).success);
    let list2 = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(!list2.contains(&a), "{list2}");
    assert!(list2.contains(&b), "{list2}");
    let bad = sb.mind(&["config", "lobes", "remove", &a]);
    assert!(!bad.success);
    assert!(
        bad.stderr.contains("not a configured agent home"),
        "{}",
        bad.stderr
    );
}

#[test]
fn config_show_creates_default_and_reports_lobes() {
    // spec: CLI-110, STO-15
    let sb = Sandbox::new();
    let cfg_path = sb.mind_home.join("config.toml");
    assert!(!cfg_path.exists());

    // show creates the config with the default lobe (the claude home).
    let show = sb.mind(&["config", "show"]);
    assert!(show.success, "{}", show.stderr);
    assert!(cfg_path.exists(), "config should be created on show");
    assert!(show.stdout.contains("config.toml"), "{}", show.stdout);
    assert!(show.stdout.contains("lobes"), "{}", show.stdout);
    assert!(
        show.stdout.contains(&sb.claude_home.display().to_string()),
        "default lobe should be the claude home: {}",
        show.stdout
    );

    // After adding a lobe, show lists it too.
    let home = sb.base.join("shownLobe").display().to_string();
    assert!(sb.mind(&["config", "lobes", "add", &home]).success);
    assert!(sb.mind(&["config", "show"]).stdout.contains(&home));
}

#[test]
fn completions_emit_a_shell_script() {
    // spec: CLI-120
    let sb = Sandbox::new();
    let r = sb.mind(&["completions", "bash"]);
    assert!(r.success, "{}", r.stderr);
    // A bash completion script registers a completion function for `mind`.
    assert!(r.stdout.contains("_mind"), "{}", r.stdout);
    assert!(r.stdout.contains("complete"), "{}", r.stdout);

    // An unknown shell is rejected by the arg parser.
    assert!(!sb.mind(&["completions", "tcsh"]).success);
}

#[test]
fn man_page_renders_roff() {
    // spec: CLI-121
    let sb = Sandbox::new();
    let r = sb.mind(&["man"]);
    assert!(r.success, "{}", r.stderr);
    // roff man pages open with a .TH title header.
    assert!(r.stdout.contains(".TH"), "{}", r.stdout);
    assert!(r.stdout.to_lowercase().contains("mind"), "{}", r.stdout);
    // The subcommand pages the top-level SUBCOMMANDS section references are
    // present in the same output rather than dangling.
    assert!(
        r.stdout.contains(".TH mind-meld") || r.stdout.contains(".TH MIND-MELD"),
        "mind-meld page must follow the top-level page: {}",
        &r.stdout[..r.stdout.len().min(2000)]
    );
    assert!(
        r.stdout.contains(".TH mind-learn") || r.stdout.contains(".TH MIND-LEARN"),
        "mind-learn page must follow the top-level page"
    );
}

// ---- concurrency tests -------------------------------------------------------
