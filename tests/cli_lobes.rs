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
        self.run_cwd(args, envs, None)
    }

    fn run_cwd(&self, args: &[&str], envs: &[(&str, &str)], cwd: Option<&Path>) -> Run {
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
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
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

// HARN-4: the windsurf preset adds its specific parent path and kinds end-to-end
// through the CLI. windsurf is skill-only.
#[test]
fn preset_add_windsurf() {
    // spec: HARN-4
    let sb = Sandbox::new();
    let added = sb.mind(&["config", "lobes", "add", "--preset", "windsurf"]);
    assert!(added.success, "windsurf add failed: {}", added.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let entry = v["lobes"]
        .as_array()
        .expect("lobes array")
        .iter()
        .find(|l| l["path"].as_str().is_some_and(|p| p.ends_with(".windsurf")))
        .unwrap_or_else(|| panic!("a .windsurf lobe entry for windsurf: {}", listed.stdout));
    let kinds: Vec<&str> = entry["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["skill"], "windsurf kinds");
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

// HARN-11 / CLI-198: `link-project <dir>` (default preset = windsurf) registers a
// project-scoped windsurf lobe at `<dir>/.windsurf`, then a `learn` fans the skill
// into it. Rules (Claude-only) must NOT appear in the windsurf lobe (HARN-10).
#[test]
fn link_project_adds_windsurf_lobe_and_fans_skill() {
    // spec: HARN-11 CLI-198
    let sb = Sandbox::new();
    let proj = sb.base.join("myproject");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // `mind link-project <proj>` -- no --preset means windsurf by default (HARN-11).
    let r = sb.mind(&["link-project", &proj_str]);
    assert!(r.success, "link-project failed: {}", r.stderr);

    // Config must record the windsurf lobe at <proj>/.windsurf with kinds=[skill].
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let ws_entry = v["lobes"]
        .as_array()
        .expect("lobes array")
        .iter()
        .find(|l| l["path"].as_str() == Some(ws_path.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "windsurf lobe at {ws_path} must be in config: {}",
                listed.stdout
            )
        });
    let kinds: Vec<&str> = ws_entry["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["skill"],
        "windsurf lobe must be skill-only (HARN-10)"
    );

    // Gitignore guidance must appear in output for project-scoped lobes (HARN-11).
    assert!(
        r.stdout.contains("gitignore") || r.stdout.contains(".gitignore"),
        "link-project must print gitignore guidance: {}",
        r.stdout
    );

    // Meld + learn: skill and rule both installed.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");
    assert!(sb.mind(&["learn", "style"]).success, "learn rule");

    // Skill must be linked into the windsurf lobe.
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/skills/review")).is_ok(),
        "skill must fan into the windsurf project lobe"
    );
    // Rule must NOT be linked into the windsurf lobe (skill-only).
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/rules")).is_err()
            && std::fs::symlink_metadata(proj.join(".windsurf/rules/style.md")).is_err(),
        "rule must NOT land in the windsurf lobe (skill-only, HARN-10)"
    );
}

// HARN-10 / CLI-199: `config lobes add <proj> --preset windsurf` is equivalent to
// `link-project <proj>`, and `--subdir <rel>` creates a skill-only lobe at
// `<proj>/<rel>`. A missing base directory returns a LobeBaseMissing error.
#[test]
fn config_lobes_add_with_preset_and_subdir() {
    // spec: HARN-10 CLI-199
    let sb = Sandbox::new();
    let proj = sb.base.join("proj2");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // `config lobes add <proj> --preset windsurf` = alias for link-project.
    let r = sb.mind(&["config", "lobes", "add", &proj_str, "--preset", "windsurf"]);
    assert!(
        r.success,
        "config lobes add --preset windsurf failed: {}",
        r.stderr
    );
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    assert!(
        v["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "config lobes add --preset windsurf must register the windsurf lobe: {}",
        listed.stdout
    );

    // `--subdir .cursor` creates a skill-only lobe at <proj>/.cursor (HARN-10).
    let proj2 = sb.base.join("proj3");
    std::fs::create_dir_all(&proj2).unwrap();
    let proj2_str = proj2.to_string_lossy().into_owned();
    let r2 = sb.mind(&["config", "lobes", "add", &proj2_str, "--subdir", ".cursor"]);
    assert!(
        r2.success,
        "config lobes add --subdir failed: {}",
        r2.stderr
    );
    let cursor_path = proj2.join(".cursor").to_string_lossy().into_owned();
    let listed2 = sb.mind(&["config", "lobes", "list", "--json"]);
    let v2 = parse_json(&listed2.stdout);
    let cursor_entry = v2["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["path"].as_str() == Some(cursor_path.as_str()))
        .unwrap_or_else(|| panic!("cursor lobe must be in config: {}", listed2.stdout));
    assert_eq!(
        cursor_entry["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["skill"],
        "subdir lobe must be skill-only"
    );

    // A missing base returns LobeBaseMissing -- the binary must exit non-zero.
    let missing = sb.mind(&[
        "config",
        "lobes",
        "add",
        "/nonexistent/proj/xyz",
        "--preset",
        "windsurf",
    ]);
    assert!(!missing.success, "missing base must fail");
    assert!(
        missing.stderr.contains("nonexistent")
            || missing.stderr.contains("not found")
            || missing.stderr.contains("does not exist")
            || missing.stderr.contains("missing"),
        "error must mention the missing path: {}",
        missing.stderr
    );
}

// HARN-10 / CLI-198: `link-project` with no explicit dir defaults to cwd (windsurf
// preset).
#[test]
fn link_project_defaults_to_cwd() {
    // spec: HARN-10 CLI-198
    let sb = Sandbox::new();
    let proj = sb.base.join("proj-cwd");
    std::fs::create_dir_all(&proj).unwrap();

    // Run link-project with cwd = proj and no positional dir argument.
    let r = sb.run_cwd(&["link-project"], &[], Some(&proj));
    assert!(r.success, "link-project (cwd) failed: {}", r.stderr);

    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    assert!(
        v["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "link-project with no dir must use cwd/.windsurf: {}",
        listed.stdout
    );
}

// HARN-12: `--snapshot` on `link-project` writes frozen real-file copies to
// `<proj>/.windsurf/...`, registers NO config entry, and a subsequent `forget`
// leaves the frozen copy intact.
#[test]
fn snapshot_writes_real_files_no_lobe_registered() {
    // spec: HARN-12
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let proj = sb.base.join("snapproj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // Snapshot: write real files, no managed lobe entry.
    let r = sb.mind(&["link-project", &proj_str, "--snapshot"]);
    assert!(r.success, "link-project --snapshot failed: {}", r.stderr);
    assert!(
        r.stdout.contains("frozen") || r.stdout.contains("wrote"),
        "snapshot must report frozen files: {}",
        r.stdout
    );

    // The skill dir must exist as a REAL directory (not a symlink).
    let skill_dir = proj.join(".windsurf/skills/review");
    let md = std::fs::metadata(&skill_dir);
    assert!(
        md.is_ok(),
        "skill dir must exist after snapshot: {}",
        r.stdout
    );
    // On Linux, `metadata` follows symlinks; a symlink to a dir has `is_dir()` too.
    // Use `symlink_metadata` to distinguish real dirs from symlinks.
    let sym_md = std::fs::symlink_metadata(&skill_dir).unwrap();
    assert!(
        !sym_md.file_type().is_symlink(),
        "snapshot must write a real dir, not a symlink"
    );

    // Config must NOT have a windsurf lobe entry (no managed registration).
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    assert!(
        !v["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "snapshot must NOT register a managed lobe: {}",
        listed.stdout
    );

    // `forget review` must succeed and the frozen copy must REMAIN (unmanaged).
    let forget = sb.mind(&["forget", "review", "--yes"]);
    assert!(forget.success, "forget failed: {}", forget.stderr);
    assert!(
        skill_dir.exists(),
        "snapshot copy must survive forget (it is not a managed link)"
    );
}

// HARN-12: `config lobes remove <path> --snapshot` converts managed symlinks in
// the lobe to frozen real-file copies and drops the config entry.
#[test]
fn lobe_remove_snapshot_converts_symlinks_to_real_files() {
    // spec: HARN-12
    let sb = Sandbox::new();
    let proj = sb.base.join("rmsnap");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // Register the lobe, meld+learn so there are symlinks inside it.
    assert!(sb.mind(&["link-project", &proj_str]).success);
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    // learn with --yes so backfill fires for the new lobe.
    assert!(
        sb.mind(&["learn", "review", "--yes"]).success,
        "learn skill"
    );

    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let skill_link = proj.join(".windsurf/skills/review");

    // Symlink must be present before removal.
    assert!(
        std::fs::symlink_metadata(&skill_link).is_ok(),
        "skill symlink must exist in the windsurf lobe before remove"
    );
    let pre_sym_md = std::fs::symlink_metadata(&skill_link).unwrap();
    assert!(
        pre_sym_md.file_type().is_symlink(),
        "pre-remove: the skill entry must be a symlink"
    );

    // Remove with --snapshot: converts and drops.
    let r = sb.mind(&["config", "lobes", "remove", &ws_path, "--snapshot"]);
    assert!(r.success, "lobe remove --snapshot failed: {}", r.stderr);
    assert!(
        r.stdout.contains("frozen") || r.stdout.contains("real"),
        "remove --snapshot must report frozen links: {}",
        r.stdout
    );

    // The entry is gone from config.
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    assert!(
        !v["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "windsurf lobe must be gone from config after remove --snapshot: {}",
        listed.stdout
    );

    // The skill dir is now a real directory, not a symlink.
    assert!(
        skill_link.exists(),
        "skill dir must still exist after remove --snapshot"
    );
    let post_sym_md = std::fs::symlink_metadata(&skill_link).unwrap();
    assert!(
        !post_sym_md.file_type().is_symlink(),
        "remove --snapshot must convert the symlink to a real file/dir"
    );
}

// HARN-13: `introspect` reports a `vanished-lobe` finding when a configured lobe's
// parent dir no longer exists, but does NOT auto-fix it.
#[test]
fn introspect_reports_vanished_lobe() {
    // spec: HARN-13
    let sb = Sandbox::new();
    let proj = sb.base.join("vanished-proj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // Register a lobe and install items so there are manifest links inside it.
    assert!(sb.mind(&["link-project", &proj_str]).success);
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "review", "--yes"]).success,
        "learn skill"
    );

    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let skill_link = proj.join(".windsurf/skills/review");
    assert!(
        std::fs::symlink_metadata(&skill_link).is_ok(),
        "skill link must exist before vanishing the dir"
    );

    // Vanish the project directory.
    std::fs::remove_dir_all(&proj).unwrap();

    // `introspect` (no --fix) must report the vanished-lobe finding without error.
    let r = sb.mind(&["introspect", "--json"]);
    assert!(
        r.success,
        "introspect must succeed even with a vanished lobe: {}",
        r.stderr
    );
    let v = parse_json(&r.stdout);
    let issues = v["issues"].as_array().expect("issues array");
    assert!(
        issues
            .iter()
            .any(|i| i["kind"].as_str() == Some("vanished-lobe")),
        "introspect must report vanished-lobe finding: {}",
        r.stdout
    );
    let vanished = issues
        .iter()
        .find(|i| i["kind"].as_str() == Some("vanished-lobe"))
        .unwrap();
    assert_eq!(
        vanished["target"].as_str(),
        Some(ws_path.as_str()),
        "vanished-lobe target must be the windsurf lobe path"
    );

    // Config must still have the vanished lobe (no --fix, no pruning).
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&listed.stdout);
    assert!(
        lv["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "config must retain the vanished lobe entry until --fix: {}",
        listed.stdout
    );
}

// HARN-13: `introspect --fix` prunes a vanished lobe from config and strips its
// links from the manifest.
#[test]
fn introspect_fix_prunes_vanished_lobe() {
    // spec: HARN-13
    let sb = Sandbox::new();
    let proj = sb.base.join("vanished-fix");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    assert!(sb.mind(&["link-project", &proj_str]).success);
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "review", "--yes"]).success,
        "learn skill"
    );

    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();

    // Confirm the manifest has a link under the windsurf lobe before vanishing.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    assert!(
        manifest["items"]["skill:review"]["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().map(|s| s.starts_with(&ws_path)).unwrap_or(false)),
        "manifest must record a link inside the windsurf lobe before fix"
    );

    // Vanish the project directory.
    std::fs::remove_dir_all(&proj).unwrap();

    // `introspect --fix` must prune the config entry and strip the manifest links.
    let r = sb.mind(&["introspect", "--fix", "--json"]);
    assert!(r.success, "introspect --fix must succeed: {}", r.stderr);

    // Config must no longer contain the vanished lobe.
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&listed.stdout);
    assert!(
        !lv["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "config must NOT contain the vanished lobe after --fix: {}",
        listed.stdout
    );

    // Manifest must no longer have links under the vanished lobe path.
    let manifest_after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let links_after = manifest_after["items"]["skill:review"]["links"]
        .as_array()
        .unwrap();
    assert!(
        !links_after
            .iter()
            .any(|l| l.as_str().map(|s| s.starts_with(&ws_path)).unwrap_or(false)),
        "manifest must not retain links under the vanished lobe after --fix: {manifest_after:#?}"
    );
}

// HARN-5: when a project-scoped preset (windsurf) is detected, `config lobes
// detect` prints guidance to run `mind link-project` but does NOT add a lobe
// (even with --yes). The JSON output includes a `guidance` field.
#[test]
fn detect_project_scoped_preset_prints_guidance_not_add() {
    // spec: HARN-5
    let sb = Sandbox::new();
    let detect_home = sb.base.join("detect-ws");
    // windsurf detection marker is .codeium/windsurf.
    std::fs::create_dir_all(detect_home.join(".codeium/windsurf")).unwrap();
    let detect_str = detect_home.to_string_lossy().into_owned();

    // Non-TTY without --yes: no mutation, guidance printed.
    let r = sb.mind_env(
        &["config", "lobes", "detect"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(r.success, "detect failed: {}", r.stderr);
    assert!(
        r.stdout.contains("link-project"),
        "detect of windsurf must print link-project guidance: {}",
        r.stdout
    );

    // Config must still be empty (no windsurf lobe added).
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let has_windsurf = v["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["path"].as_str().is_some_and(|p| p.ends_with(".windsurf")));
    assert!(
        !has_windsurf,
        "detect must NOT add a project-scoped windsurf lobe: {}",
        listed.stdout
    );

    // --yes must also NOT add the windsurf lobe (project-scoped).
    let r_yes = sb.mind_env(
        &["config", "lobes", "detect", "--yes"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(r_yes.success, "detect --yes failed: {}", r_yes.stderr);
    let listed2 = sb.mind(&["config", "lobes", "list", "--json"]);
    let v2 = parse_json(&listed2.stdout);
    let has_windsurf2 = v2["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["path"].as_str().is_some_and(|p| p.ends_with(".windsurf")));
    assert!(
        !has_windsurf2,
        "detect --yes must NOT auto-add a project-scoped preset: {}",
        listed2.stdout
    );

    // JSON output must include windsurf in `detected` with scope=project and a
    // non-empty `guidance` array.
    let r_json = sb.mind_env(
        &["config", "lobes", "detect", "--json"],
        &[("MIND_DETECT_HOME", &detect_str)],
    );
    assert!(r_json.success, "detect --json failed: {}", r_json.stderr);
    let jv = parse_json(&r_json.stdout);
    assert!(
        jv["detected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["preset"] == "windsurf" && d["scope"] == "project"),
        "JSON detected must include windsurf with scope=project: {}",
        r_json.stdout
    );
    assert!(
        jv["guidance"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "JSON must include a non-empty guidance array for project-scoped presets: {}",
        r_json.stdout
    );
}

// HARN-12: `--snapshot` refuses to clobber a pre-existing FOREIGN target (one mind
// did not place) without `--force`, leaving it intact; with `--force` it overwrites
// the foreign target with the frozen skill copy.
#[test]
fn snapshot_force_overwrites_foreign_target() {
    // spec: HARN-12
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let proj = sb.base.join("forcesnap");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // Plant a foreign FILE exactly where the frozen skill dir would land.
    let target = proj.join(".windsurf/skills/review");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "foreign content").unwrap();

    // Without --force: the collision is refused and the foreign file is untouched.
    let refused = sb.mind(&["link-project", &proj_str, "--snapshot"]);
    assert!(
        !refused.success,
        "snapshot must refuse a foreign target without --force: stdout={} stderr={}",
        refused.stdout, refused.stderr
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "foreign content",
        "the foreign file must be left intact when the snapshot is refused"
    );

    // With --force: the frozen skill dir overwrites the foreign file.
    let forced = sb.mind(&["link-project", &proj_str, "--snapshot", "--force"]);
    assert!(
        forced.success,
        "snapshot --force must overwrite the foreign target: {}",
        forced.stderr
    );
    assert!(
        target.join("SKILL.md").is_file(),
        "with --force the frozen skill dir must replace the foreign file: {}",
        forced.stdout
    );
    // And it is a REAL directory, not a symlink (snapshot writes real files).
    let md = std::fs::symlink_metadata(&target).unwrap();
    assert!(
        !md.file_type().is_symlink() && md.file_type().is_dir(),
        "the frozen skill must be a real directory after --force"
    );
}

// HARN-11: re-adding an already-registered lobe is an idempotent no-op. The text
// no-op path ("already configured") is covered elsewhere; this pins the `--json`
// output shape (action=lobe-add, outcome=no-op), and that nothing is duplicated.
#[test]
fn lobe_re_add_json_reports_no_op() {
    // spec: HARN-11
    let sb = Sandbox::new();
    let proj = sb.base.join("reproj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // First add via link-project (windsurf) registers <proj>/.windsurf.
    assert!(
        sb.mind(&["link-project", &proj_str]).success,
        "first link-project must succeed"
    );

    // Re-add the SAME lobe through the equivalent `config lobes add ... --preset
    // windsurf --json`: it must report a no-op, not a second entry.
    let second = sb.mind(&[
        "config", "lobes", "add", &proj_str, "--preset", "windsurf", "--json",
    ]);
    assert!(
        second.success,
        "re-adding a registered lobe must succeed: {}",
        second.stderr
    );
    let v = parse_json(&second.stdout);
    assert_eq!(v["action"], "lobe-add", "{}", second.stdout);
    assert_eq!(
        v["outcome"], "no-op",
        "re-adding an already-registered lobe must be a no-op: {}",
        second.stdout
    );

    // The lobe list still shows exactly one windsurf entry.
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&listed.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let count = lv["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["path"].as_str() == Some(ws_path.as_str()))
        .count();
    assert_eq!(
        count, 1,
        "no-op re-add must not duplicate the lobe: {}",
        listed.stdout
    );
}

// HARN-9: `link-project` as the FIRST explicit lobe add on an otherwise-default
// (implicit ~/.claude) config auto-preserves claude_home, so a later install still
// reaches ~/.claude in addition to the new project lobe.
#[test]
fn link_project_first_add_preserves_claude_home() {
    // spec: HARN-9
    let sb = Sandbox::new();
    // No write_config: the lobes config is the implicit claude_home default.
    let proj = sb.base.join("firstlink");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    assert!(
        sb.mind(&["link-project", &proj_str]).success,
        "first link-project must succeed"
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    // The skill must reach BOTH claude_home (auto-preserved) and the project lobe.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "claude_home must be preserved as an explicit lobe after the first link-project"
    );
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/skills/review")).is_ok(),
        "the skill must reach the project windsurf lobe"
    );
}

// HARN-10: the windsurf skill-only `kinds` filter rejects BOTH an agent and a tool
// (existing tests only checked a rule). Installing a skill + agent + tool must
// materialize ONLY the skill into the windsurf project lobe.
#[test]
fn windsurf_skill_only_rejects_agent_and_tool() {
    // spec: HARN-10
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-ws-kinds-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let source = base.join("agents");
    write(
        &source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\n# review\n",
    );
    write(
        &source.join("helper.md"),
        "---\nname: helper\ndescription: an agent\n---\n# helper agent\n",
    );
    write(&source.join("toolkit/run.sh"), "#!/bin/sh\necho hi\n");
    write(
        &source.join("mind.toml"),
        "[source]\ndescription = \"multi-kind source\"\n\n\
         [[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"skills/review\"\n\n\
         [[items]]\nkind = \"agent\"\nname = \"helper\"\npath = \"helper.md\"\n\n\
         [[items]]\nkind = \"tool\"\nname = \"toolkit\"\npath = \"toolkit\"\nlink = \"tools/toolkit\"\n",
    );
    git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&source, &["config", "user.email", "t@t"]);
    git(&source, &["config", "user.name", "t"]);
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "initial"]);

    let mind_home = base.join("mind");
    let claude_home = base.join("claude");
    std::fs::create_dir_all(&mind_home).unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();

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

    // Register the windsurf project lobe, meld, then learn all three kinds.
    assert!(run(&["link-project", proj.to_str().unwrap()]).success);
    assert!(run(&["meld", source.to_str().unwrap()]).success);
    assert!(run(&["learn", "review"]).success, "learn skill");
    let la = run(&["learn", "helper"]);
    assert!(la.success, "learn agent failed: {}", la.stderr);
    let lt = run(&["learn", "toolkit"]);
    assert!(lt.success, "learn tool failed: {}", lt.stderr);

    // Only the skill lands in the windsurf lobe.
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/skills/review")).is_ok(),
        "skill must land in the windsurf lobe"
    );
    // The agent must NOT (skill-only lobe rejects agents).
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/agents")).is_err()
            && std::fs::symlink_metadata(proj.join(".windsurf/agents/helper.md")).is_err(),
        "an agent must NOT land in a skill-only windsurf lobe (HARN-10)"
    );
    // The tool (even with an explicit link) must NOT.
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/tools")).is_err()
            && std::fs::symlink_metadata(proj.join(".windsurf/tools/toolkit")).is_err(),
        "a tool must NOT land in a skill-only windsurf lobe (HARN-10)"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// CLI-198: `link-project --json` (managed, non-snapshot) emits a well-formed
// lobe-add mutation result and does NOT leak the prose gitignore note into stdout.
#[test]
fn link_project_json_reports_mutation() {
    // spec: CLI-198
    let sb = Sandbox::new();
    let proj = sb.base.join("jsonproj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    let r = sb.mind(&["link-project", &proj_str, "--json"]);
    assert!(r.success, "link-project --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "lobe-add", "{}", r.stdout);
    assert_eq!(v["outcome"], "added", "{}", r.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    assert_eq!(
        v["target"].as_str(),
        Some(ws_path.as_str()),
        "target must be the windsurf lobe path: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("gitignore"),
        "JSON mode must not emit the prose gitignore note: {}",
        r.stdout
    );
}

// HARN-12: `--snapshot` with `--json` still performs the freeze without crashing.
// Per HARN-12 the snapshot output contract is the prose "wrote N frozen skill(s)"
// line (there is no JSON mutation schema defined for snapshot), so this asserts the
// freeze happened and no managed lobe was registered rather than a JSON shape.
#[test]
fn snapshot_json_still_freezes_and_registers_no_lobe() {
    // spec: HARN-12
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success, "learn skill");

    let proj = sb.base.join("snapjson");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    let r = sb.mind(&["link-project", &proj_str, "--snapshot", "--json"]);
    assert!(r.success, "snapshot --json failed: {}", r.stderr);

    // The freeze happened: a real (non-symlink) skill dir exists.
    let skill_dir = proj.join(".windsurf/skills/review");
    let md = std::fs::symlink_metadata(&skill_dir).expect("frozen skill dir must exist");
    assert!(
        !md.file_type().is_symlink() && md.file_type().is_dir(),
        "snapshot must write a real dir under --json too"
    );

    // No managed lobe registered (snapshot never writes a config entry).
    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let lv = parse_json(&listed.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    assert!(
        !lv["lobes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["path"].as_str() == Some(ws_path.as_str())),
        "snapshot must not register a managed lobe even under --json: {}",
        listed.stdout
    );
}

// HARN-12: `--snapshot` when nothing is installed is a no-op note, not an error,
// and it creates no skill directory.
#[test]
fn snapshot_no_items_is_noop_not_error() {
    // spec: HARN-12
    let sb = Sandbox::new();
    let proj = sb.base.join("emptysnap");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    // Nothing has been learned; snapshot must succeed with a no-op note.
    let r = sb.mind(&["link-project", &proj_str, "--snapshot"]);
    assert!(
        r.success,
        "snapshot with nothing installed must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no installed items"),
        "empty snapshot must print the no-items note: {}",
        r.stdout
    );
    assert!(
        std::fs::symlink_metadata(proj.join(".windsurf/skills")).is_err(),
        "no skills dir must be created when there is nothing to snapshot"
    );
}

// CLI-198: running `link-project <same dir>` twice is idempotent: the second run
// succeeds, reports "already configured", and does not duplicate the config entry.
#[test]
fn link_project_twice_is_idempotent() {
    // spec: CLI-198
    let sb = Sandbox::new();
    let proj = sb.base.join("twiceproj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj_str = proj.to_string_lossy().into_owned();

    assert!(
        sb.mind(&["link-project", &proj_str]).success,
        "first link-project must succeed"
    );
    let second = sb.mind(&["link-project", &proj_str]);
    assert!(
        second.success,
        "second link-project (same dir) must succeed: {}",
        second.stderr
    );
    assert!(
        second.stdout.contains("already configured"),
        "second link-project must report the lobe already configured: {}",
        second.stdout
    );

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let count = v["lobes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["path"].as_str() == Some(ws_path.as_str()))
        .count();
    assert_eq!(
        count, 1,
        "the windsurf lobe must appear exactly once after two link-projects: {}",
        listed.stdout
    );
}

// STO-16: a RELATIVE project base given to `config lobes add ... --preset windsurf`
// is resolved to an absolute path in the saved config (resolved against cwd).
#[test]
fn lobe_add_relative_base_resolves_absolute() {
    // spec: STO-16
    let sb = Sandbox::new();
    let proj = sb.base.join("relproj");
    std::fs::create_dir_all(&proj).unwrap();

    // Run with cwd = sb.base and a RELATIVE base "relproj".
    let r = sb.run_cwd(
        &["config", "lobes", "add", "relproj", "--preset", "windsurf"],
        &[],
        Some(&sb.base),
    );
    assert!(r.success, "relative-base add failed: {}", r.stderr);

    let listed = sb.mind(&["config", "lobes", "list", "--json"]);
    let v = parse_json(&listed.stdout);
    let has_abs = v["lobes"].as_array().unwrap().iter().any(|l| {
        l["path"]
            .as_str()
            .map(|p| p.starts_with('/') && p.ends_with("relproj/.windsurf"))
            .unwrap_or(false)
    });
    assert!(
        has_abs,
        "a relative base must resolve to an absolute windsurf lobe path (STO-16): {}",
        listed.stdout
    );
}

// HARN-12: `config lobes remove --snapshot` on a lobe holding MULTIPLE items freezes
// every managed link to a real file AND leaves the ~/.mind store copies intact (the
// freeze copies FROM the store, it must not consume it).
#[test]
fn lobe_remove_snapshot_multiple_items_keeps_store() {
    // spec: HARN-12
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-rmsnap-multi-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let source = base.join("agents");
    write(
        &source.join("skills/one/SKILL.md"),
        "---\nname: one\ndescription: skill one\n---\n# one\n",
    );
    write(
        &source.join("skills/two/SKILL.md"),
        "---\nname: two\ndescription: skill two\n---\n# two\n",
    );
    git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&source, &["config", "user.email", "t@t"]);
    git(&source, &["config", "user.name", "t"]);
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "initial"]);

    let mind_home = base.join("mind");
    let claude_home = base.join("claude");
    std::fs::create_dir_all(&mind_home).unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();

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

    assert!(run(&["link-project", proj.to_str().unwrap()]).success);
    assert!(run(&["meld", source.to_str().unwrap()]).success);
    assert!(run(&["learn", "one"]).success, "learn one");
    assert!(run(&["learn", "two"]).success, "learn two");

    let ws_path = proj.join(".windsurf").to_string_lossy().into_owned();
    let link_one = proj.join(".windsurf/skills/one");
    let link_two = proj.join(".windsurf/skills/two");
    assert!(
        std::fs::symlink_metadata(&link_one)
            .unwrap()
            .file_type()
            .is_symlink()
            && std::fs::symlink_metadata(&link_two)
                .unwrap()
                .file_type()
                .is_symlink(),
        "both skills must be symlinked into the windsurf lobe before remove"
    );

    // Capture the store paths so we can assert they survive the freeze.
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let store_one = manifest["items"]["skill:one"]["store"]
        .as_str()
        .unwrap()
        .to_string();
    let store_two = manifest["items"]["skill:two"]["store"]
        .as_str()
        .unwrap()
        .to_string();

    let r = run(&["config", "lobes", "remove", &ws_path, "--snapshot"]);
    assert!(r.success, "remove --snapshot failed: {}", r.stderr);

    // Both links are now REAL directories, not symlinks.
    for link in [&link_one, &link_two] {
        let md = std::fs::symlink_metadata(link).unwrap();
        assert!(
            !md.file_type().is_symlink() && md.file_type().is_dir(),
            "remove --snapshot must convert {link:?} to a real dir"
        );
        assert!(
            link.join("SKILL.md").is_file(),
            "the frozen copy at {link:?} must contain the skill file"
        );
    }

    // The store copies must remain intact (the freeze copies FROM the store).
    assert!(
        mind_home.join(&store_one).join("SKILL.md").is_file(),
        "store copy for skill:one must survive the freeze"
    );
    assert!(
        mind_home.join(&store_two).join("SKILL.md").is_file(),
        "store copy for skill:two must survive the freeze"
    );

    let _ = std::fs::remove_dir_all(&base);
}
