//! Cross-harness lobes (spec/harness-lobes.md, HARN-1..6): end-to-end tests that
//! drive the real `mind` binary against a hermetic, network-free fixture. They
//! cover the per-lobe `kinds` filter, the `--preset` add, the `kinds`-aware
//! display, and the auto-detect-and-prompt setup (via `MIND_DETECT_HOME`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway environment: a source git repo plus isolated MIND/CLAUDE homes.
struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    /// A source repo with one skill (`review`) and one rule (`style`), committed.
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-lobes-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("agents");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(
            &source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\n",
        );
        write(
            &source.join("rules/style.md"),
            "---\ndescription: ASCII only\n---\n# style rule\n",
        );
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        std::fs::create_dir_all(&sb.mind_home).unwrap();
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.run(args, &[])
    }

    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.run(args, envs)
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// Write `config.toml` verbatim into the mind home (so a test can pin the
    /// exact lobe set, including a `kinds`-filtered table entry).
    fn write_config(&self, body: &str) {
        write(&self.mind_home.join("config.toml"), body);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(repo: &Path, args: &[&str]) {
    std::fs::create_dir_all(repo).unwrap();
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn parse_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("not JSON: {e}\n{stdout}"))
}

// HARN-4: `config lobes add --preset gemini` records the preset's parent path
// and `kinds` filter as a detailed lobe entry.
#[test]
fn preset_add_records_path_and_kinds() {
    // spec: HARN-4
    let sb = Sandbox::new();
    let added = sb.mind(&["config", "lobes", "add", "--preset", "gemini"]);
    assert!(added.success, "preset add failed: {}", added.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let lobes = v["lobes"].as_array().expect("lobes array");
    let gemini = lobes
        .iter()
        .find(|l| {
            l["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(".gemini/config"))
        })
        .expect("a .gemini/config lobe entry");
    let kinds: Vec<&str> = gemini["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["skill"], "gemini is skill-only");

    // An unknown preset name is rejected.
    let bad = sb.mind(&["config", "lobes", "add", "--preset", "emacs"]);
    assert!(!bad.success, "unknown preset must fail");
    assert!(bad.stderr.contains("preset"), "{}", bad.stderr);

    // Removed presets are also unknown.
    let ag = sb.mind(&["config", "lobes", "add", "--preset", "antigravity"]);
    assert!(!ag.success, "removed antigravity preset must fail");
    let agcli = sb.mind(&["config", "lobes", "add", "--preset", "antigravity-cli"]);
    assert!(!agcli.success, "removed antigravity-cli preset must fail");

    // Supplying both a path and --preset is rejected by the CLI.
    let both = sb.mind(&["config", "lobes", "add", "/tmp/x", "--preset", "gemini"]);
    assert!(!both.success, "path + --preset must conflict");
}

// HARN-2 / HARN-3: a skill links into a `[skill]`-only lobe AND the all-kinds
// Claude lobe, while a rule links ONLY into the all-kinds lobe. The rule's
// manifest `links` omit the skill-only lobe, and no rule file lands there.
#[test]
fn kinds_filter_excludes_rule_from_skill_only_lobe() {
    // spec: HARN-2
    // spec: HARN-3
    // spec: HARN-6
    let sb = Sandbox::new();
    let skill_lobe = sb.base.join("gemini-lobe");
    sb.write_config(&format!(
        "lobes = [\"{claude}\", {{ path = \"{skill}\", kinds = [\"skill\"] }}]\n",
        claude = sb.claude_home.display(),
        skill = skill_lobe.display(),
    ));

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");
    assert!(sb.mind(&["learn", "style"]).success, "learn rule");

    // The skill is linked into BOTH lobes (skill is admitted everywhere).
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "skill must link into the Claude lobe"
    );
    assert!(
        std::fs::symlink_metadata(skill_lobe.join("skills/review")).is_ok(),
        "skill must link into the skill-only lobe"
    );

    // The rule links ONLY into the all-kinds Claude lobe, never the skill-only one.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("rules/style")).is_ok()
            || std::fs::symlink_metadata(sb.claude_home.join("rules/style.md")).is_ok(),
        "rule must link into the Claude lobe"
    );
    assert!(
        std::fs::symlink_metadata(skill_lobe.join("rules/style.md")).is_err(),
        "rule must NOT link into a skill-only lobe (HARN-3)"
    );

    // The recorded manifest links reflect exactly the admitted lobes (HARN-2).
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let rule_links: Vec<&str> = manifest["items"]["rule:style"]["links"]
        .as_array()
        .expect("rule links array")
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    let skill_lobe_str = skill_lobe.display().to_string();
    assert!(
        !rule_links.iter().any(|l| l.starts_with(&skill_lobe_str)),
        "rule manifest links must omit the skill-only lobe: {rule_links:?}"
    );
    assert!(
        rule_links
            .iter()
            .any(|l| l.starts_with(&sb.claude_home.display().to_string())),
        "rule manifest links must include the Claude lobe: {rule_links:?}"
    );

    // HARN-6: the linked skill file is verbatim - the frontmatter is not rewritten
    // for the non-Claude lobe.
    let linked = std::fs::read_to_string(skill_lobe.join("skills/review/SKILL.md")).unwrap();
    let original = std::fs::read_to_string(sb.source.join("skills/review/SKILL.md")).unwrap();
    assert_eq!(
        linked, original,
        "mind links skill/agent files verbatim; no frontmatter rewrite (HARN-6)"
    );
}

// HARN-1: `config lobes list` and `config show` surface a lobe's kinds filter.
#[test]
fn list_and_show_display_kinds() {
    // spec: HARN-1
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["config", "lobes", "add", "--preset", "gemini"])
            .success
    );

    let list = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(
        list.contains(".gemini/config") && list.contains("skill"),
        "list must show the kinds filter: {list}"
    );

    let show = sb.mind(&["config", "show"]).stdout;
    assert!(
        show.contains(".gemini/config") && show.contains("skill"),
        "show must surface the kinds filter: {show}"
    );
}

// HARN-5: `config lobes detect` reports the harness homes that exist under the
// detection base; without `--yes` (and non-TTY) it never mutates config; with
// `--yes` it adds the detected lobes. `--json` is machine-readable.
#[test]
fn detect_reports_then_yes_adds() {
    // spec: HARN-5
    let sb = Sandbox::new();
    // A detection base with .gemini present but no other harness markers.
    let detect_home = sb.base.join("detect");
    std::fs::create_dir_all(detect_home.join(".gemini")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    // Report-only: a non-TTY run without --yes must NOT mutate config.
    let report = sb.mind_env(
        &["config", "lobes", "detect"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(report.success, "detect failed: {}", report.stderr);
    assert!(
        report.stdout.contains("gemini"),
        "detect must report gemini: {}",
        report.stdout
    );
    let after_report = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&after_report.stdout);
    assert!(
        !v["lobes"].as_array().unwrap().iter().any(|l| l["path"]
            .as_str()
            .is_some_and(|p| p.ends_with(".gemini/config"))),
        "report-only detect must NOT add the lobe: {}",
        after_report.stdout
    );

    // --json is machine-readable and lists the detected preset.
    let json = sb.mind_env(
        &["config", "lobes", "detect", "--json"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    let jv = parse_json(&json.stdout);
    assert_eq!(jv["action"], "lobe-detect", "{}", json.stdout);
    assert_eq!(
        jv["added"], false,
        "json report must not mutate: {}",
        json.stdout
    );
    assert!(
        jv["detected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["preset"] == "gemini"),
        "detected must list gemini: {}",
        json.stdout
    );

    // --yes adds the detected lobe(s) under the detection base.
    let added = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(added.success, "detect --yes failed: {}", added.stderr);
    let after_add = sb.mind(&["config", "lobes", "list", "--json"]);
    let av = parse_json(&after_add.stdout);
    let gemini = av["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| {
            l["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(".gemini/config"))
        })
        .expect("detect --yes must add the gemini/config lobe");
    assert_eq!(
        gemini["path"].as_str().unwrap(),
        detect_home.join(".gemini/config").display().to_string(),
        "added lobe path must be under the detection base"
    );
}

// HARN-5: the `--yes` NON-JSON text path reports each added lobe in human output
// and actually persists it to config (the JSON --yes path is covered above; this
// pins the plain-text branch the implementor flagged).
#[test]
fn detect_yes_text_output_reports_and_persists() {
    // spec: HARN-5
    let sb = Sandbox::new();
    let detect_home = sb.base.join("detect");
    std::fs::create_dir_all(detect_home.join(".gemini")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    let added = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(added.success, "detect --yes failed: {}", added.stderr);
    assert!(
        added.stdout.contains("gemini") && added.stdout.contains("added"),
        "text --yes must report the added gemini lobe: {}",
        added.stdout
    );

    // It must have persisted: a second non-JSON list shows the .gemini/config lobe.
    let list = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(
        list.contains(".gemini/config"),
        "detect --yes must persist the lobe: {list}"
    );
}

// HARN-5: when no known harness dirs exist under the detection base, detect
// reports "no new harness homes detected" and mutates nothing - in both the text
// and JSON forms, with and without --yes (the empty-candidates path the
// implementor flagged).
#[test]
fn detect_no_homes_reports_nothing_and_mutates_nothing() {
    // spec: HARN-5
    let sb = Sandbox::new();
    // An empty detection base: no harness markers at all.
    let detect_home = sb.base.join("empty-detect");
    std::fs::create_dir_all(&detect_home).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    // Text form, even with --yes, says nothing was found and adds nothing.
    let text = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(text.success, "detect failed: {}", text.stderr);
    assert!(
        text.stdout.contains("no new harness homes"),
        "empty detection must report none found: {}",
        text.stdout
    );

    // JSON form: detected is empty and added is false.
    let json = sb.mind_env(
        &["config", "lobes", "detect", "--json", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    let jv = parse_json(&json.stdout);
    assert_eq!(jv["action"], "lobe-detect");
    assert_eq!(
        jv["added"], false,
        "no candidates => added=false: {}",
        json.stdout
    );
    assert!(
        jv["detected"].as_array().unwrap().is_empty(),
        "detected must be empty: {}",
        json.stdout
    );

    // Config still has no lobes added.
    let list = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&list.stdout);
    // The default-only listing is the claude home (a single default entry), never
    // a harness lobe.
    assert!(
        !lv["lobes"].as_array().unwrap().iter().any(|l| l["path"]
            .as_str()
            .is_some_and(|p| p.ends_with(".gemini/config") || p.ends_with(".agents"))),
        "empty detection must not have added any harness lobe: {}",
        list.stdout
    );
}

// HARN-2/HARN-4/HARN-5: codex and universal both resolve to ~/.agents. When the
// detection base has BOTH `.codex` and `.agents` markers, detect must collapse
// the two same-path candidates to a single .agents lobe (first-seen wins) rather
// than offering/adding the same path twice. Drives the binary end-to-end.
#[test]
fn detect_dedups_codex_and_universal_same_path() {
    // spec: HARN-2
    // spec: HARN-5
    let sb = Sandbox::new();
    let detect_home = sb.base.join("detect-dup");
    // Both markers present: .codex (codex preset) and .agents (universal preset).
    std::fs::create_dir_all(detect_home.join(".codex")).unwrap();
    std::fs::create_dir_all(detect_home.join(".agents")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    let json = sb.mind_env(
        &["config", "lobes", "detect", "--json"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    let jv = parse_json(&json.stdout);
    let agents_path = detect_home.join(".agents").display().to_string();
    let agents_entries: Vec<&serde_json::Value> = jv["detected"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["path"].as_str() == Some(agents_path.as_str()))
        .collect();
    assert_eq!(
        agents_entries.len(),
        1,
        "codex+universal must collapse to ONE ~/.agents candidate: {}",
        json.stdout
    );

    // And adding them persists exactly one .agents lobe.
    let added = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(added.success, "{}", added.stderr);
    let list = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&list.stdout);
    let agents_lobes: Vec<&serde_json::Value> = lv["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["path"].as_str() == Some(agents_path.as_str()))
        .collect();
    assert_eq!(
        agents_lobes.len(),
        1,
        "only one ~/.agents lobe must be persisted: {}",
        list.stdout
    );
}

// HARN-4: the codex preset adds its specific parent path and kinds end-to-end
// through the CLI (not just the unit lookup). codex is skill-only.
#[test]
fn preset_add_codex() {
    // spec: HARN-4
    let sb = Sandbox::new();
    let added = sb.mind(&["config", "lobes", "add", "--preset", "codex"]);
    assert!(added.success, "codex add failed: {}", added.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let entry = v["lobes"]
        .as_array()
        .expect("lobes array")
        .iter()
        .find(|l| l["path"].as_str().is_some_and(|p| p.ends_with(".agents")))
        .unwrap_or_else(|| panic!("a .agents lobe entry for codex: {}", listed.stdout));
    let kinds: Vec<&str> = entry["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["skill"], "codex kinds");
}

// HARN-2: a kinds-filtered lobe survives the item lifecycle. Install a skill
// (admitted everywhere) and a rule (Claude-only) into a [skill]-only lobe plus
// the all-kinds Claude lobe, then:
//  - `introspect` reports NO drift/missing-link for the intentionally-absent
//    rule link in the skill-only lobe (it operates on RECORDED links).
//  - `forget` removes exactly the recorded links and errors on nothing, even
//    though the rule was never linked into the skill-only lobe.
#[test]
fn kinds_filtered_lobe_lifecycle_forget_and_introspect() {
    // spec: HARN-2
    let sb = Sandbox::new();
    let skill_lobe = sb.base.join("skill-only-lobe");
    sb.write_config(&format!(
        "lobes = [\"{claude}\", {{ path = \"{skill}\", kinds = [\"skill\"] }}]\n",
        claude = sb.claude_home.display(),
        skill = skill_lobe.display(),
    ));

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");
    assert!(sb.mind(&["learn", "style"]).success, "learn rule");

    // introspect must be clean: the rule was never linked into the skill-only
    // lobe, so it must NOT be reported as a missing link.
    let intro = sb.mind(&["introspect", "--json"]);
    assert!(intro.success, "introspect failed: {}", intro.stderr);
    let iv = parse_json(&intro.stdout);
    let issues = iv["issues"].as_array().expect("issues array");
    assert!(
        issues.is_empty(),
        "a kinds-filtered lobe must produce no drift/missing-link issues: {}",
        intro.stdout
    );

    // forget the rule: it removes the recorded Claude link, leaves the skill-only
    // lobe untouched, and reports success with no error about a missing rule link.
    let forget_rule = sb.mind(&["forget", "style", "--yes"]);
    assert!(
        forget_rule.success,
        "forget of a rule in a kinds-filtered setup must succeed: {}",
        forget_rule.stderr
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("rules/style.md")).is_err(),
        "the recorded Claude rule link must be removed by forget"
    );
    // The skill is still linked in both lobes (it was admitted everywhere).
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok()
            && std::fs::symlink_metadata(skill_lobe.join("skills/review")).is_ok(),
        "the skill must remain linked in both lobes after forgetting the rule"
    );

    // forget the skill: both recorded links go away cleanly.
    let forget_skill = sb.mind(&["forget", "review", "--yes"]);
    assert!(forget_skill.success, "{}", forget_skill.stderr);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err()
            && std::fs::symlink_metadata(skill_lobe.join("skills/review")).is_err(),
        "forget must remove the skill from every recorded lobe"
    );

    // Manifest is now empty of these items; introspect is still clean.
    let intro2 = sb.mind(&["introspect", "--json"]);
    let iv2 = parse_json(&intro2.stdout);
    assert!(
        iv2["issues"].as_array().unwrap().is_empty(),
        "introspect must stay clean after forget: {}",
        intro2.stdout
    );
}

// HARN-2: `upgrade` re-installs through the same kinds-aware link planning, so an
// upstream content change to a rule re-links only into the lobes that admit it
// (never the skill-only lobe), and produces no error about the absent link.
#[test]
fn upgrade_respects_kinds_filter() {
    // spec: HARN-2
    let sb = Sandbox::new();
    let skill_lobe = sb.base.join("skill-only-lobe");
    sb.write_config(&format!(
        "lobes = [\"{claude}\", {{ path = \"{skill}\", kinds = [\"skill\"] }}]\n",
        claude = sb.claude_home.display(),
        skill = skill_lobe.display(),
    ));

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "style"]).success, "learn rule");

    // Change the rule upstream and commit, then sync so the catalog sees it.
    write(
        &sb.source.join("rules/style.md"),
        "---\ndescription: ASCII only\n---\n# style rule v2\n",
    );
    git(&sb.source, &["commit", "-aqm", "bump rule"]);
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        up.success,
        "upgrade of a kinds-filtered rule must succeed: {}",
        up.stderr
    );

    // The rule still links only into the Claude lobe, never the skill-only one.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("rules/style.md")).is_ok(),
        "rule must remain linked in the Claude lobe after upgrade"
    );
    assert!(
        std::fs::symlink_metadata(skill_lobe.join("rules/style.md")).is_err(),
        "rule must NOT be linked into the skill-only lobe after upgrade (HARN-2)"
    );

    // The recorded links still omit the skill-only lobe.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let rule_links: Vec<&str> = manifest["items"]["rule:style"]["links"]
        .as_array()
        .expect("rule links")
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    let skill_lobe_str = skill_lobe.display().to_string();
    assert!(
        !rule_links.iter().any(|l| l.starts_with(&skill_lobe_str)),
        "rule links must still omit the skill-only lobe after upgrade: {rule_links:?}"
    );
}

// HARN-3 / `config lobes` UX: a path supplied without `--preset` is fine, but
// supplying NEITHER a path nor `--preset` is the LobeTargetRequired error. Driven
// through the binary (the dispatch arm the implementor flagged as untested).
#[test]
fn lobes_add_without_path_or_preset_errors() {
    // spec: HARN-4
    let sb = Sandbox::new();
    let run = sb.mind(&["config", "lobes", "add"]);
    assert!(!run.success, "add with no target must fail");
    assert!(
        run.stderr.contains("path") && run.stderr.contains("--preset"),
        "error must mention both a path and --preset: {}",
        run.stderr
    );
}

// HARN-4: backward compat. An existing `lobes = ["~/.claude"]` style config (a
// bare string entry) must SURVIVE a `config lobes add --preset` rewrite as a
// bare string - the rewrite must not promote it to a table. The preset entry is
// added as a table with its kinds.
#[test]
fn preset_add_preserves_bare_entry_shape() {
    // spec: HARN-1
    // spec: HARN-4
    let sb = Sandbox::new();
    // Seed a bare-entry config exactly as a pre-feature install would have.
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));

    let added = sb.mind(&["config", "lobes", "add", "--preset", "gemini"]);
    assert!(added.success, "preset add failed: {}", added.stderr);

    // Read the rewritten config.toml verbatim and assert the bare entry stayed
    // bare (a quoted string), while the preset entry is a table with kinds.
    let raw = std::fs::read_to_string(sb.mind_home.join("config.toml")).unwrap();
    let bare = format!("\"{}\"", sb.claude_home.display());
    assert!(
        raw.contains(&bare),
        "the original bare lobe must remain a bare string after rewrite:\n{raw}"
    );
    assert!(
        raw.contains("kinds") && raw.contains(".gemini/config"),
        "the preset lobe must be a table with kinds:\n{raw}"
    );

    // And the JSON list shows the bare entry as a plain string (its shape-
    // preserving serialization), while the preset entry is an object with kinds.
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let lobes = v["lobes"].as_array().unwrap();
    let claude_str = sb.claude_home.display().to_string();
    assert!(
        lobes
            .iter()
            .any(|l| l.as_str() == Some(claude_str.as_str())),
        "a bare lobe must serialize as a plain JSON string (all-kinds): {}",
        listed.stdout
    );
    assert!(
        lobes.iter().any(|l| l.is_object()
            && l["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(".gemini/config"))
            && l["kinds"].is_array()),
        "the preset lobe must serialize as an object with a kinds array: {}",
        listed.stdout
    );
}

// HARN-4: `config lobes remove` drops a preset-added (detailed table) lobe by its
// path - removal keys on the path regardless of whether the entry carries a
// kinds filter.
#[test]
fn remove_preset_added_detailed_lobe_by_path() {
    // spec: HARN-4
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["config", "lobes", "add", "--preset", "gemini"])
            .success
    );

    // Find the persisted path.
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let gemini_path = v["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|l| {
            l["path"]
                .as_str()
                .filter(|p| p.ends_with(".gemini/config"))
                .map(str::to_string)
        })
        .expect("gemini lobe path");

    let removed = sb.mind(&["config", "lobes", "remove", &gemini_path]);
    assert!(
        removed.success,
        "removing a detailed preset lobe by path must succeed: {}",
        removed.stderr
    );

    let after = sb.mind(&["config", "lobes", "list", "--json"]);
    let av = parse_json(&after.stdout);
    assert!(
        !av["lobes"].as_array().unwrap().iter().any(|l| l["path"]
            .as_str()
            .is_some_and(|p| p.ends_with(".gemini/config"))),
        "the gemini lobe must be gone after remove: {}",
        after.stdout
    );
}

// HARN-1: a tool item with an EXPLICIT link still links into a no-kinds lobe
// (TOOL-4 preserved: a no-kinds lobe admits every kind, including tool), but is
// excluded from a kinds-filtered lobe that does not list `tool`.
#[test]
fn tool_with_explicit_link_respects_kinds_filter() {
    // spec: HARN-1
    // A bespoke source: an authoritative mind.toml declaring a tool with a link.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-tool-lobe-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let source = base.join("agents");
    write(&source.join("toolkit/run.sh"), "#!/bin/sh\necho hi\n");
    write(
        &source.join("mind.toml"),
        "[source]\ndescription = \"tool source\"\n\n[[items]]\nkind = \"tool\"\nname = \"toolkit\"\npath = \"toolkit\"\nlink = \"tools/toolkit\"\n",
    );
    git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&source, &["config", "user.email", "t@t"]);
    git(&source, &["config", "user.name", "t"]);
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "initial"]);

    let mind_home = base.join("mind");
    let claude_home = base.join("claude");
    std::fs::create_dir_all(&mind_home).unwrap();
    let skill_lobe = base.join("skill-only-lobe");

    let run = |args: &[&str]| -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &mind_home)
            .env("CLAUDE_HOME", &claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    };

    // Config: a no-kinds Claude lobe plus a [skill]-only lobe.
    write(
        &mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{claude}\", {{ path = \"{skill}\", kinds = [\"skill\"] }}]\n",
            claude = claude_home.display(),
            skill = skill_lobe.display(),
        ),
    );

    assert!(run(&["meld", &source.to_string_lossy()]).success);
    let learned = run(&["learn", "toolkit"]);
    assert!(learned.success, "learn tool failed: {}", learned.stderr);

    // The tool's explicit link lands in the no-kinds Claude lobe (TOOL-4).
    assert!(
        std::fs::symlink_metadata(claude_home.join("tools/toolkit")).is_ok(),
        "a tool with an explicit link must link into a no-kinds lobe (TOOL-4)"
    );
    // But NOT into the skill-only lobe (it does not admit `tool`).
    assert!(
        std::fs::symlink_metadata(skill_lobe.join("tools/toolkit")).is_err(),
        "a skill-only lobe must not receive a tool link (HARN-1)"
    );

    // Manifest links record only the admitted (Claude) lobe.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let links: Vec<&str> = manifest["items"]["tool:toolkit"]["links"]
        .as_array()
        .expect("tool links")
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    let skill_lobe_str = skill_lobe.display().to_string();
    assert!(
        !links.iter().any(|l| l.starts_with(&skill_lobe_str)),
        "tool links must omit the skill-only lobe: {links:?}"
    );
    assert!(
        links
            .iter()
            .any(|l| l.starts_with(&claude_home.display().to_string())),
        "tool links must include the no-kinds Claude lobe: {links:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// CLI-150: `mind config lobes detect -y` must parse and run (not error with
// "unexpected argument '-y'"). With no detected homes the command exits 0.
#[test]
fn detect_short_yes_flag_is_accepted() {
    // spec: CLI-150
    let sb = Sandbox::new();
    // Empty detection base: no harness markers, so detect exits cleanly.
    let detect_home = sb.base.join("detect-empty");
    std::fs::create_dir_all(&detect_home).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    // Post-verb short flag: `mind config lobes detect -y`
    let run = sb.mind_env(
        &["config", "lobes", "detect", "-y"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(
        run.success,
        "detect -y must succeed (not error on the flag): stderr={}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("unexpected argument"),
        "detect -y must not produce 'unexpected argument': {}",
        run.stderr
    );
}

// CLI-150: `mind -y config lobes detect` must parse correctly; the global -y
// must be the source of the confirmation flag, not a local field that would
// silently ignore the pre-verb position.
#[test]
fn detect_global_yes_pre_verb_is_accepted() {
    // spec: CLI-150
    let sb = Sandbox::new();
    let detect_home = sb.base.join("detect-empty2");
    std::fs::create_dir_all(&detect_home).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    // Pre-verb global flag: `mind -y config lobes detect`
    let run = sb.mind_env(
        &["-y", "config", "lobes", "detect"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(
        run.success,
        "mind -y config lobes detect must succeed: stderr={}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("unexpected argument"),
        "mind -y detect must not error on the flag: {}",
        run.stderr
    );

    // Also verify pre-verb --yes works the same way.
    let detect_home2 = sb.base.join("detect-empty3");
    std::fs::create_dir_all(&detect_home2).unwrap();
    let detect_str2 = detect_home2.to_string_lossy().into_owned();
    let run2 = sb.mind_env(
        &["--yes", "config", "lobes", "detect"],
        &[("MIND_DETECT_HOME", &detect_str2)],
    );
    assert!(
        run2.success,
        "mind --yes config lobes detect must succeed: stderr={}",
        run2.stderr
    );
}

// HARN-7: `config lobes add --yes <path>` backfills already-installed items into
// the newly-added lobe without prompting.
#[test]
fn lobe_add_backfills_installed_items_with_yes() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    let added = sb.mind(&["config", "lobes", "add", "--yes", &new_lobe_str]);
    assert!(added.success, "lobe add --yes failed: {}", added.stderr);

    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "the installed skill must be backfilled into the new lobe: {}",
        added.stdout
    );
}

// HARN-7: `config lobes add --preset <name> --yes` backfills installed items into
// the preset's lobe.
#[test]
fn lobe_add_preset_backfills_with_yes() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    // Pin HOME to the sandbox so the gemini preset resolves under it (never the
    // real home), keeping the backfilled symlink hermetic.
    let home_str = sb.base.to_string_lossy().into_owned();
    let added = sb.mind_env(
        &["config", "lobes", "add", "--preset", "gemini", "--yes"],
        &[("HOME", &home_str)],
    );
    assert!(added.success, "preset add --yes failed: {}", added.stderr);

    assert!(
        std::fs::symlink_metadata(sb.base.join(".gemini/config/skills/review")).is_ok(),
        "the installed skill must be backfilled into the gemini preset lobe: {}",
        added.stdout
    );
}

// HARN-7: `config lobes detect --yes` backfills installed items into a detected
// harness lobe.
#[test]
fn lobe_detect_backfills_with_yes() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let detect_home = sb.base.join("detect");
    std::fs::create_dir_all(detect_home.join(".gemini")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();
    let added = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(added.success, "detect --yes failed: {}", added.stderr);

    assert!(
        std::fs::symlink_metadata(detect_home.join(".gemini/config/skills/review")).is_ok(),
        "the installed skill must be backfilled into the detected gemini lobe: {}",
        added.stdout
    );
}

// HARN-7: non-interactive (non-TTY) `config lobes add` without `--yes` prints a
// note pointing at `introspect --fix` and does NOT backfill.
#[test]
fn lobe_add_no_tty_no_yes_prints_note() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    let added = sb.mind(&["config", "lobes", "add", &new_lobe_str]);
    assert!(added.success, "lobe add failed: {}", added.stderr);

    assert!(
        added.stdout.contains("mind introspect --fix"),
        "non-TTY add without --yes must print the introspect note: {}",
        added.stdout
    );
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_err(),
        "without --yes the skill must NOT be backfilled into the new lobe"
    );
}

// HARN-8: `introspect` (without --fix) reports a lobe whose installed items were
// never linked into it as a `missing-lobe-link` finding.
#[test]
fn introspect_reports_missing_lobe_links() {
    // spec: HARN-8
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    // Add a second lobe without backfilling (non-TTY, no --yes): the config now
    // carries it but the installed skill is not linked there.
    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add failed"
    );

    let intro = sb.mind(&["introspect", "--json"]);
    let iv = parse_json(&intro.stdout);
    let issues = iv["issues"].as_array().expect("issues array");
    assert!(
        issues.iter().any(|i| i["kind"] == "missing-lobe-link"),
        "introspect must report the uncovered lobe as missing-lobe-link: {}",
        intro.stdout
    );
}

// HARN-8: `introspect --fix` creates the missing lobe links, updates the
// manifest, and leaves no missing-lobe-link finding on a re-run.
#[test]
fn introspect_fix_creates_missing_lobe_links() {
    // spec: HARN-8
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add failed"
    );

    let fixed = sb.mind(&["introspect", "--fix"]);
    assert!(fixed.success, "introspect --fix failed: {}", fixed.stderr);
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "introspect --fix must create the missing lobe link: {}",
        fixed.stdout
    );

    // A re-run is clean: the manifest now records the link, so no finding remains.
    let intro = sb.mind(&["introspect", "--json"]);
    let iv = parse_json(&intro.stdout);
    let issues = iv["issues"].as_array().expect("issues array");
    assert!(
        !issues.iter().any(|i| i["kind"] == "missing-lobe-link"),
        "after --fix no missing-lobe-link finding must remain: {}",
        intro.stdout
    );
}

// HARN-7: adding a lobe when nothing is installed skips the backfill silently
// (no prompt, no note) and still reports the added lobe.
#[test]
fn lobe_add_no_items_skips_backfill_silently() {
    // spec: HARN-7
    let sb = Sandbox::new();
    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    let added = sb.mind(&["config", "lobes", "add", &new_lobe_str]);
    assert!(added.success, "lobe add failed: {}", added.stderr);

    assert!(
        added.stdout.contains("added lobe"),
        "add must confirm the lobe: {}",
        added.stdout
    );
    assert!(
        !added.stdout.contains("mind introspect --fix"),
        "with nothing installed there is no backfill note: {}",
        added.stdout
    );
}

// HARN-7: `config lobes add --json <path>` without `--yes` must skip backfill
// silently in JSON mode — no "introspect" prose note in stdout, only valid JSON.
#[test]
fn harn7_json_no_yes_skips_backfill_silently() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("newlobe-json-noskip");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    // Non-TTY, no --yes, but in --json mode.
    let run = sb.mind(&["config", "lobes", "add", "--json", &new_lobe_str]);
    assert!(run.success, "lobe add --json failed: {}", run.stderr);

    // Output must be parseable JSON (not prose).
    let v = parse_json(&run.stdout);
    assert_eq!(
        v["action"], "lobe-add",
        "JSON must include action: {}",
        run.stdout
    );
    assert_eq!(v["outcome"], "added", "lobe must be added: {}", run.stdout);

    // The "introspect --fix" note must be suppressed in JSON mode.
    assert!(
        !run.stdout.contains("introspect"),
        "JSON mode must not emit the introspect note: {}",
        run.stdout
    );

    // The item must NOT be backfilled (JSON + no --yes = silent skip).
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_err(),
        "without --yes the skill must NOT be backfilled in JSON mode"
    );
}

// HARN-7: `config lobes add --json --yes <path>` backfills already-installed items
// silently (no prose) and emits valid JSON as its only stdout output.
#[test]
fn harn7_json_yes_backfills_silently() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("newlobe-json-yes");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    let run = sb.mind(&["config", "lobes", "add", "--json", "--yes", &new_lobe_str]);
    assert!(run.success, "lobe add --json --yes failed: {}", run.stderr);

    // Output must be parseable JSON.
    let v = parse_json(&run.stdout);
    assert_eq!(v["action"], "lobe-add", "{}", run.stdout);
    assert_eq!(v["outcome"], "added", "{}", run.stdout);

    // The item IS backfilled (JSON + --yes = silent backfill).
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "with --yes the skill must be backfilled in JSON mode: {}",
        run.stdout
    );
}

// HARN-7: backfill via a skill-only preset (codex) respects the `kinds` filter:
// the skill is linked into the new lobe but the rule is NOT.
#[test]
fn harn7_backfill_preset_codex_skill_only_excludes_rule() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");
    assert!(sb.mind(&["learn", "style"]).success, "learn rule");

    // Add the codex preset (skill-only, resolves to ~/.agents) with --yes to
    // trigger immediate backfill. Pin HOME to the sandbox base so the preset
    // resolves hermetically under it.
    let home_str = sb.base.to_string_lossy().into_owned();
    let added = sb.mind_env(
        &["config", "lobes", "add", "--preset", "codex", "--yes"],
        &[("HOME", &home_str)],
    );
    assert!(added.success, "preset add --yes failed: {}", added.stderr);

    // The skill IS backfilled into the codex lobe (<base>/.agents/skills/review).
    assert!(
        std::fs::symlink_metadata(sb.base.join(".agents/skills/review")).is_ok(),
        "the skill must be backfilled into the codex (skill-only) lobe"
    );
    // The rule is NOT linked: codex preset is skill-only and excludes rules.
    assert!(
        std::fs::symlink_metadata(sb.base.join(".agents/rules/style.md")).is_err(),
        "the rule must NOT be backfilled into a skill-only lobe (HARN-1/HARN-7)"
    );
}

// HARN-7: adding a lobe that is ALREADY configured (no-op path) does NOT trigger
// backfill; only newly-added lobes receive the backfill offer.
#[test]
fn harn7_no_op_lobe_add_does_not_backfill() {
    // spec: HARN-7
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("existing-lobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();

    // First add: newly-added lobe receives backfill with --yes.
    let first = sb.mind(&["config", "lobes", "add", "--yes", &new_lobe_str]);
    assert!(first.success, "{}", first.stderr);
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "first add must backfill the skill"
    );

    // Remove the link so we can detect if a second add re-creates it.
    std::fs::remove_file(new_lobe.join("skills/review")).unwrap();

    // Second add (same path) is a no-op: lobe already configured.
    let second = sb.mind(&["config", "lobes", "add", "--yes", &new_lobe_str]);
    assert!(second.success, "{}", second.stderr);
    assert!(
        second.stdout.contains("already configured"),
        "must report lobe already configured: {}",
        second.stdout
    );
    // The link was NOT recreated: no-op add does not trigger backfill.
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_err(),
        "no-op add must not backfill items into an already-configured lobe"
    );
}

// HARN-8: when a foreign file at the expected link's parent directory blocks link
// creation during `introspect --fix`, the fix emits a `missing-lobe-link` finding
// but still exits 0 and does not abort processing of other items or lobes.
#[test]
fn harn8_introspect_fix_clobber_reports_finding_and_continues() {
    // spec: HARN-8
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    // Add a second lobe without backfill (non-TTY, no --yes).
    let new_lobe = sb.base.join("blocked-lobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add failed"
    );

    // Plant a regular FILE at `new_lobe/skills` so that mkdir_p inside
    // ensure_link fails with ENOTDIR when it tries to create the skills/ dir.
    std::fs::create_dir_all(&new_lobe).unwrap();
    std::fs::write(new_lobe.join("skills"), "blocking file").unwrap();

    // introspect --fix must report the clobber but still exit 0.
    let run = sb.mind(&["introspect", "--fix", "--json"]);
    assert!(
        run.success,
        "introspect --fix must not abort on a clobber conflict: {}",
        run.stderr
    );

    let v = parse_json(&run.stdout);
    let issues = v["issues"].as_array().expect("issues array");
    assert!(
        issues.iter().any(|i| i["kind"] == "missing-lobe-link"),
        "a blocked link must produce a missing-lobe-link finding: {}",
        run.stdout
    );

    // The skill is still NOT linked (creation failed).
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_err(),
        "the blocked link must remain absent after --fix"
    );

    // The original skill symlink in claude_home must survive the clobber error
    // and remain intact -- only the new blocked lobe is affected. spec: HARN-8
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "the original claude_home skill symlink must be intact after a blocked lobe error (HARN-8)"
    );
}

// HARN-8: `introspect --fix` must report exactly ONE `missing-lobe-link` finding per
// blocked link -- the error propagated from `link_into_new_lobes`, and no additional
// redundant finding from a second `symlink_metadata` check.
//
#[test]
fn harn8_introspect_fix_reports_exactly_one_finding_per_blocked_link() {
    // spec: HARN-8
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let new_lobe = sb.base.join("blocked-lobe-count");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add failed"
    );

    // Plant a regular FILE at `new_lobe/skills` so mkdir_p fails with ENOTDIR.
    std::fs::create_dir_all(&new_lobe).unwrap();
    std::fs::write(new_lobe.join("skills"), "blocking file").unwrap();

    let run = sb.mind(&["introspect", "--fix", "--json"]);
    assert!(
        run.success,
        "introspect --fix must not abort: {}",
        run.stderr
    );

    let v = parse_json(&run.stdout);
    let issues = v["issues"].as_array().expect("issues array");
    let count = issues
        .iter()
        .filter(|i| i["kind"] == "missing-lobe-link")
        .count();
    assert_eq!(
        count, 1,
        "exactly one missing-lobe-link finding per blocked link, got {count}: {}",
        run.stdout
    );
}

// HARN-8: when a recorded link is in the manifest but absent from disk (broken
// symlink), `introspect` (without --fix) fires BOTH `missing-link` and
// `missing-lobe-link` for that link. This documents the current behavior: the
// first fires from the manifest-vs-disk check; the second fires from the HARN-8
// lobe-coverage check (on-disk test fails). With `--fix`, relink restores the
// symlink and neither finding fires on a re-run.
#[test]
fn harn8_broken_recorded_link_fires_both_findings_without_fix() {
    // spec: HARN-8
    let sb = Sandbox::new();
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    // Delete the skill symlink from disk; it remains in the manifest.
    // The link is a symlink, so remove_file removes the link itself (not the store).
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();

    // Without --fix: both missing-link and missing-lobe-link fire for the same path.
    let intro = sb.mind(&["introspect", "--json"]);
    let iv = parse_json(&intro.stdout);
    let issues = iv["issues"].as_array().expect("issues array");
    assert!(
        issues.iter().any(|i| i["kind"] == "missing-link"),
        "a broken recorded link must fire a missing-link finding: {}",
        intro.stdout
    );
    assert!(
        issues.iter().any(|i| i["kind"] == "missing-lobe-link"),
        "a broken recorded link also fires missing-lobe-link (on-disk check, HARN-8): {}",
        intro.stdout
    );

    // With --fix: relink restores the symlink; a re-run finds no issues.
    let fixed = sb.mind(&["introspect", "--fix", "--json"]);
    assert!(fixed.success, "{}", fixed.stderr);
    let fv = parse_json(&fixed.stdout);
    let fixed_issues = fv["issues"].as_array().expect("issues array");
    assert!(
        fixed_issues.is_empty(),
        "after --fix no issues must remain for a broken recorded link: {}",
        fixed.stdout
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "the symlink must be restored by --fix"
    );
}

// HARN-8: the real user scenario -- items installed with the implicit default
// (no lobes in config), then lobes added, then `introspect --fix` backfills
// them. The `link_rel` recovery falls back to `default_link_rel` because
// the recorded links are in claude_home, which is NOT in agent_homes() once an
// explicit lobe list is configured.
#[test]
fn introspect_fix_backfills_after_implicit_default_install() {
    // spec: HARN-8
    let sb = Sandbox::new();
    // NOTE: no write_config here -- install goes to the implicit claude_home default.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "skill must be linked into implicit claude_home"
    );

    // Add a new lobe (no --yes; HARN-7 prints the note, backfill is deferred).
    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add failed"
    );

    // introspect --fix must create the missing link in the new lobe.
    let fixed = sb.mind(&["introspect", "--fix"]);
    assert!(fixed.success, "introspect --fix failed: {}", fixed.stderr);
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "introspect --fix must create the missing lobe link: {}",
        fixed.stdout
    );

    // A re-run must be clean.
    let intro = sb.mind(&["introspect", "--json"]);
    let iv = parse_json(&intro.stdout);
    let issues = iv["issues"].as_array().expect("issues array");
    assert!(
        !issues.iter().any(|i| i["kind"] == "missing-lobe-link"),
        "after --fix no missing-lobe-link must remain: {}",
        intro.stdout
    );
}

// HARN-9: when the first explicit lobe is added to an empty lobes config,
// claude_home is auto-preserved so that new installs still reach it.
#[test]
fn first_lobe_add_preserves_claude_home() {
    // spec: HARN-9
    let sb = Sandbox::new();
    // No write_config: lobes config starts empty (implicit claude_home default).
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let new_lobe = sb.base.join("newlobe");
    let new_lobe_str = new_lobe.to_string_lossy().into_owned();
    assert!(
        sb.mind(&["config", "lobes", "add", &new_lobe_str]).success,
        "lobe add must succeed"
    );

    // Install an item after the lobe add.
    assert!(
        sb.mind(&["learn", "review"]).success,
        "learn after lobe-add failed"
    );

    // Item must be in BOTH claude_home (preserved implicit default) and new_lobe.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "new install must reach claude_home after first lobe-add"
    );
    assert!(
        std::fs::symlink_metadata(new_lobe.join("skills/review")).is_ok(),
        "new install must reach the explicitly-added lobe"
    );
}

// HARN-9: `config lobes add` when claude_home itself is the path does not
// create a duplicate entry.
#[test]
fn lobe_add_claude_home_no_duplicate() {
    // spec: HARN-9
    let sb = Sandbox::new();
    let ch = sb.claude_home.to_string_lossy().into_owned();
    let added = sb.mind(&["config", "lobes", "add", &ch]);
    assert!(
        added.success,
        "lobe add claude_home failed: {}",
        added.stderr
    );

    // lobe list must show exactly one entry (not claude_home twice).
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&listed.stdout);
    let lobes = lv["lobes"].as_array().expect("lobes array");
    assert_eq!(
        lobes.len(),
        1,
        "adding claude_home must not create a duplicate: {}",
        listed.stdout
    );
}

// HARN-9: `config lobes add --preset gemini` on an explicit empty lobes config
// (`lobes = []`) auto-prepends claude_home before saving the new lobe. This
// exercises the `cfg.lobes.is_empty()` branch -- distinct from the no-config-file
// case, which goes through `ensure_config` and already has claude_home in the list.
#[test]
fn preset_add_preserves_claude_home_on_empty_lobes_config() {
    // spec: HARN-9
    let sb = Sandbox::new();
    // Explicit empty lobes list to trigger the cfg.lobes.is_empty() HARN-9 branch.
    sb.write_config("lobes = []\n");

    // Pin HOME so the gemini preset resolves hermetically under the sandbox.
    let home_str = sb.base.to_string_lossy().into_owned();
    let added = sb.mind_env(
        &["config", "lobes", "add", "--preset", "gemini"],
        &[("HOME", &home_str)],
    );
    assert!(added.success, "preset add failed: {}", added.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let lobes = v["lobes"].as_array().expect("lobes array");

    // Must have two saved entries: claude_home (auto-prepended) + gemini preset.
    assert_eq!(
        lobes.len(),
        2,
        "claude_home must be auto-prepended before the gemini preset on empty config: {}",
        listed.stdout
    );
    let ch = sb.claude_home.to_string_lossy().into_owned();
    assert!(
        lobes.iter().any(|l| {
            // bare entries serialize as plain JSON strings
            l.as_str() == Some(ch.as_str()) || l["path"].as_str() == Some(ch.as_str())
        }),
        "claude_home must appear as first entry in the saved lobe list: {}",
        listed.stdout
    );
}

// HARN-9: `config lobes detect --yes` on an explicit empty lobes config
// (`lobes = []`) auto-prepends claude_home before saving the detected lobes.
// Mirrors the preset path but exercises the `lobe_detect` HARN-9 branch.
#[test]
fn detect_yes_preserves_claude_home_on_empty_lobes_config() {
    // spec: HARN-9
    let sb = Sandbox::new();
    // Explicit empty lobes list to trigger the cfg.lobes.is_empty() HARN-9 branch.
    sb.write_config("lobes = []\n");

    // Create a .gemini marker dir under the detect base so the gemini preset is found.
    let detect_home = sb.base.join("detect");
    std::fs::create_dir_all(detect_home.join(".gemini")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();
    let added = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(added.success, "detect --yes failed: {}", added.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let lobes = v["lobes"].as_array().expect("lobes array");

    // Must have two saved entries: claude_home (auto-prepended) + detected gemini lobe.
    assert_eq!(
        lobes.len(),
        2,
        "claude_home must be auto-prepended by detect --yes on empty config: {}",
        listed.stdout
    );
    let ch = sb.claude_home.to_string_lossy().into_owned();
    assert!(
        lobes.iter().any(|l| {
            // bare entries serialize as plain JSON strings
            l.as_str() == Some(ch.as_str()) || l["path"].as_str() == Some(ch.as_str())
        }),
        "claude_home must appear in the saved lobe list after detect: {}",
        listed.stdout
    );
}
