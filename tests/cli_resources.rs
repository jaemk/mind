//! The `resource` item kind (spec/resources.md) and path-token dependency
//! edges (spec/dependencies.md DEP-8): end-to-end tests that drive the real
//! `mind` binary against a hermetic fixture (a local git repo melded by path,
//! isolated MIND_HOME/CLAUDE_HOME). No network.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
    /// An empty source directory; the caller writes files, then [`commit`].
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-res-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("lib");
        std::fs::create_dir_all(&source).unwrap();
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
    }

    /// The motivating layout: a skill reading a JSON allowlist and a template
    /// directory, the template reading a footer, plus entries nothing
    /// references (an install script, a dot-file, a symlink).
    fn standard() -> Sandbox {
        let sb = Sandbox::new();
        sb.write(
            "skills/review/SKILL.md",
            "---\ndescription: review\n---\nallow {{resources:allowlist.json}}\n\
             template {{resources:templates}}/pr.md\n",
        );
        sb.write(
            "skills/plain/SKILL.md",
            "---\ndescription: plain\n---\nbody\n",
        );
        sb.write("resources/allowlist.json", "{\"allow\": []}\n");
        sb.write(
            "resources/templates/pr.md",
            "# PR\n\nfooter: {{resources:footer.md}}\n",
        );
        sb.write("resources/footer.md", "thanks\n");
        sb.write("resources/install.sh", "#!/bin/sh\n");
        sb.write("resources/.gitkeep", "");
        std::os::unix::fs::symlink("allowlist.json", sb.source.join("resources/linked.json"))
            .unwrap();
        sb.commit();
        sb
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.source.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn commit(&self) {
        for args in [
            vec!["-c", "init.defaultBranch=main", "init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
            vec!["add", "-A"],
            vec!["commit", "-qm", "fixture"],
        ] {
            let status = Command::new("git")
                .args(&args)
                .current_dir(&self.source)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?}");
        }
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_TTY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Meld without installing anything: with no `--yes` and stdin at EOF,
    /// the install offer reads as "no".
    fn meld_only(&self, extra: &[&str]) {
        let spec = self.spec();
        let mut args = vec!["meld", spec.as_str()];
        args.extend_from_slice(extra);
        let r = self.mind(&args);
        assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    }

    /// Each `probe --json` row as `kind:name`.
    fn catalog_keys(&self) -> Vec<String> {
        let r = self.mind(&["probe", "--json"]);
        assert!(r.success, "probe: {}", r.stderr);
        let v: Value = serde_json::from_str(&r.stdout).expect("probe --json is JSON");
        let rows = v["items"].as_array().expect("probe --json items");
        let keys: Vec<String> = rows
            .iter()
            .map(|row| {
                format!(
                    "{}:{}",
                    row["kind"].as_str().unwrap_or_default(),
                    row["name"].as_str().unwrap_or_default()
                )
            })
            .collect();
        assert!(!keys.is_empty(), "the catalog is not empty: {v}");
        keys
    }

    fn manifest(&self) -> Value {
        let raw = std::fs::read_to_string(self.mind_home.join("manifest.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn store(&self) -> PathBuf {
        self.mind_home.join("store")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn has(keys: &[String], key: &str) -> bool {
    keys.iter().any(|k| k == key)
}

#[test]
fn convention_resources_are_offered_only_when_referenced() {
    // spec: RES-1 RES-2 RES-3
    let sb = Sandbox::standard();
    sb.meld_only(&[]);
    let keys = sb.catalog_keys();
    // Referenced directly by the skill, by full file name.
    assert!(has(&keys, "resource:allowlist.json"), "{keys:?}");
    assert!(has(&keys, "resource:templates"), "{keys:?}");
    // Referenced only by a kept resource (templates/pr.md).
    assert!(has(&keys, "resource:footer.md"), "{keys:?}");
    // Nothing references these; a dot-entry and a symlink are never resources.
    for absent in [
        "resource:install.sh",
        "resource:.gitkeep",
        "resource:linked.json",
    ] {
        assert!(
            !has(&keys, absent),
            "{absent} must not be offered: {keys:?}"
        );
    }
}

#[test]
fn learn_pulls_in_referenced_resources_store_only_and_expanded() {
    // spec: RES-6 RES-10 RES-12 DEP-8
    let sb = Sandbox::standard();
    sb.meld_only(&[]);
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);

    let store = sb.store();
    // DEP-8: the resource the skill's tokens name came along, and RES-3's
    // transitive reference too.
    assert!(store.join("resource/allowlist.json").is_file());
    assert!(store.join("resource/templates/pr.md").is_file());
    assert!(store.join("resource/footer.md").is_file());
    assert!(
        !store.join("skill/plain").exists(),
        "an unreferenced sibling stays out"
    );

    // RES-6: store-only, with an empty link set.
    assert!(!sb.claude_home.join("resources").exists());
    let manifest = sb.manifest();
    let entry = &manifest["items"]["resource:allowlist.json"];
    assert_eq!(
        entry["store"], "store/resource/allowlist.json",
        "{manifest}"
    );
    assert_eq!(entry["links"], Value::Array(vec![]), "{manifest}");

    // RES-10: the skill names the installed file and the template directory.
    let s = store.display().to_string();
    let skill = std::fs::read_to_string(store.join("skill/review/SKILL.md")).unwrap();
    assert!(
        skill.contains(&format!("allow {s}/resource/allowlist.json")),
        "{skill}"
    );
    assert!(
        skill.contains(&format!("template {s}/resource/templates/pr.md")),
        "{skill}"
    );
    // RES-12: a markdown file inside a directory resource expands like any
    // markdown item file.
    let pr = std::fs::read_to_string(store.join("resource/templates/pr.md")).unwrap();
    assert!(pr.contains(&format!("{s}/resource/footer.md")), "{pr}");

    // RES-6: forget removes the store copy.
    let f = sb.mind(&["forget", "resource:allowlist.json", "--yes"]);
    assert!(f.success, "{}\n{}", f.stdout, f.stderr);
    assert!(!store.join("resource/allowlist.json").exists());
}

#[test]
fn resource_drift_is_tracked_by_upgrade() {
    // spec: RES-6
    let sb = Sandbox::standard();
    sb.meld_only(&[]);
    assert!(sb.mind(&["learn", "skill:review", "--yes"]).success);
    sb.write("resources/allowlist.json", "{\"allow\": [\"git\"]}\n");
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "change"]] {
        let ok = Command::new("git")
            .args(&args)
            .current_dir(&sb.source)
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("allowlist.json"), "{}", r.stdout);
    let got = std::fs::read_to_string(sb.store().join("resource/allowlist.json")).unwrap();
    assert!(got.contains("git"), "{got}");
}

#[test]
fn declared_resources_are_offered_unreferenced() {
    // spec: RES-4
    let sb = Sandbox::new();
    sb.write("data/notes.txt", "notes\n");
    sb.write("assets/logo.svg", "<svg/>\n");
    sb.write("assets/icons/a.svg", "<svg/>\n");
    sb.write(
        "mind.toml",
        "[[items]]\nkind = \"resource\"\nname = \"notes\"\npath = \"data/notes.txt\"\n\
         description = \"Team notes\"\n\n\
         [discover]\nresources = { include = [\"assets/*\"] }\n",
    );
    sb.commit();
    sb.meld_only(&[]);
    let keys = sb.catalog_keys();
    assert!(has(&keys, "resource:notes"), "{keys:?}");
    // A glob match keeps its full file name; a directory match is one item.
    assert!(has(&keys, "resource:logo.svg"), "{keys:?}");
    assert!(has(&keys, "resource:icons"), "{keys:?}");
    let r = sb.mind(&["learn", "resource:notes", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(
        std::fs::read_to_string(sb.store().join("resource/notes")).unwrap(),
        "notes\n"
    );
}

#[test]
fn resource_reads_no_metadata_and_rejects_tool_and_link_fields() {
    // spec: RES-5
    let sb = Sandbox::new();
    // Frontmatter in a markdown resource is content: no description, no hook.
    sb.write(
        "resources/guide.md",
        "---\ndescription: not metadata\ninstall: touch /nonexistent/x\n---\nbody\n",
    );
    sb.write(
        "skills/review/SKILL.md",
        "---\ndescription: review\n---\nsee {{resources:guide.md}}\n",
    );
    sb.commit();
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(!r.stdout.contains("not metadata"), "{}", r.stdout);
    assert!(!r.stdout.contains("item-hook"), "{}", r.stdout);
    // No description is expected of a resource, so none is asked for.
    assert!(
        !r.stdout.contains("resource:guide.md: no description"),
        "{}",
        r.stdout
    );

    for (field, value) in [("link", "\"rules/x.md\""), ("bin", "\"run\"")] {
        let bad = Sandbox::new();
        bad.write("resources/x.md", "x\n");
        bad.write(
            "mind.toml",
            &format!(
                "[[items]]\nkind = \"resource\"\nname = \"x.md\"\npath = \"resources/x.md\"\n\
                 {field} = {value}\n"
            ),
        );
        bad.commit();
        let r = bad.mind(&["meld", &bad.spec(), "--yes"]);
        assert!(!r.success, "{field} on a resource must be refused");
        assert!(r.stderr.contains(field), "{field}: {}", r.stderr);
    }
}

#[test]
fn resource_prefix_applies_and_resource_is_a_reserved_prefix() {
    // spec: RES-7
    let sb = Sandbox::standard();
    let spec = sb.spec();
    let r = sb.mind(&["meld", &spec, "--namespace", "jk", "--yes"]);
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    let store = sb.store();
    assert!(store.join("resource/jk:allowlist.json").is_file());
    let skill = std::fs::read_to_string(store.join("skill/jk:review/SKILL.md")).unwrap();
    assert!(
        skill.contains(&format!("{}/resource/jk:allowlist.json", store.display())),
        "{skill}"
    );

    let other = Sandbox::standard();
    let r = other.mind(&["meld", &other.spec(), "--namespace", "resource", "--yes"]);
    assert!(!r.success, "`resource` is a reserved prefix word");
}

#[test]
fn resources_are_never_unmanaged_and_plugins_have_none() {
    // spec: RES-8
    // A stray directory named like the kind in an agent home is not an
    // unmanaged item: nothing scans for resources there.
    let sb = Sandbox::standard();
    std::fs::create_dir_all(sb.claude_home.join("resources/stray")).unwrap();
    std::fs::write(sb.claude_home.join("resources/stray/x"), "x").unwrap();
    let r = sb.mind(&["recall", "--json"]);
    assert!(r.success, "{}", r.stderr);
    assert!(!r.stdout.contains("stray"), "{}", r.stdout);

    // A plugin manifest source has no resource component.
    let plugin = Sandbox::new();
    plugin.write(".claude-plugin/plugin.json", "{\"name\": \"acme\"}\n");
    plugin.write(
        "skills/plain/SKILL.md",
        "---\ndescription: plain\n---\nbody\n",
    );
    plugin.write(
        "skills/reader/SKILL.md",
        "---\ndescription: reader\n---\nsee {{path:resource:data.json}}\n",
    );
    plugin.write("resources/data.json", "{}\n");
    plugin.commit();
    plugin.meld_only(&[]);
    let keys = plugin.catalog_keys();
    assert!(keys.iter().all(|k| !k.starts_with("resource:")), "{keys:?}");
}

#[test]
fn unknown_resource_token_fails_install_and_review() {
    // spec: RES-11
    let sb = Sandbox::new();
    sb.write(
        "skills/review/SKILL.md",
        "---\ndescription: review\n---\nsee {{resources:nope.json}}\n",
    );
    sb.commit();
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(!r.success);
    assert!(r.stderr.contains("bad-reference"), "{}", r.stderr);
    assert!(r.stderr.contains("{{resources:nope.json}}"), "{}", r.stderr);

    sb.meld_only(&[]);
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(!r.success);
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);
}

#[test]
fn review_reports_unreferenced_resources() {
    // spec: RES-14
    let sb = Sandbox::standard();
    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    let lines: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| l.contains("unreferenced-resource"))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "only install.sh is unreferenced: {}",
        r.stdout
    );
    assert!(lines[0].contains("resources/install.sh"), "{}", lines[0]);
    assert!(
        lines[0].contains("{{resources:install.sh}}"),
        "{}",
        lines[0]
    );
}

#[test]
fn learn_pulls_in_a_tool_named_by_a_path_token() {
    // spec: DEP-8
    let sb = Sandbox::new();
    sb.write("tools/detect/detect", "#!/bin/sh\n");
    sb.write("tools/other/other", "#!/bin/sh\n");
    sb.write(
        "skills/scan/SKILL.md",
        "---\ndescription: scan\n---\nrun {{tools:detect}}\n",
    );
    sb.commit();
    sb.meld_only(&[]);
    let r = sb.mind(&["learn", "skill:scan", "--yes"]);
    assert!(r.success, "{}\n{}", r.stdout, r.stderr);
    assert!(sb.store().join("tool/detect/detect").is_file());
    assert!(!sb.store().join("tool/other").exists());
}

#[test]
fn kind_filter_accepts_resource() {
    // spec: RES-1
    let sb = Sandbox::standard();
    sb.meld_only(&[]);
    let r = sb.mind(&["probe", "--no-tui", "--kind", "resource"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("allowlist.json"), "{}", r.stdout);
    assert!(!r.stdout.contains("skill:review"), "{}", r.stdout);
}
