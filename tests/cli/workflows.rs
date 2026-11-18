//! The `workflow` item kind (spec/workflows.md).

use super::*;

/// The cost side of WF-25's unconditional grant, which docs/src/source-layout.md
/// now documents: a `{{ns:...}}`-shaped string anywhere in a workflow's `.js` is
/// expanded and VALIDATED like any other reference, so one naming no sibling
/// fails the install hard (`BadReference`, NS-12) instead of being the inert text
/// it would be in an unlisted non-markdown file. A `.js` has no frontmatter, so
/// there is no `expand:` key to omit and no way to opt the file back out.
///
/// The existing coverage for this stops at the `review` surface (which has no
/// installed copy and expands the file itself); this drives the real install, and
/// pins the transactional consequence too: a failed install leaves no store copy
/// and no link, and the source's other items still install afterward.
#[test]
fn a_workflows_unresolvable_token_fails_the_install_and_leaves_nothing() {
    // spec: WF-25 WF-27 NS-12 LIFE-1
    let sb = Sandbox::new();
    // Not a `meta.name` token, and not prose either: a template string in the
    // body, which is the "written with no intention of it being a token" case.
    sb.write_and_commit(
        "workflows/stray.js",
        "export const meta = {\n  name: 'stray',\n  description: 'Stray token',\n}\n\
         agent(`see {{ns:nonesuch}} for the format`)\n",
    );
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );

    let learn = sb.mind(&["learn", "workflow:stray", "--yes"]);
    assert!(
        !learn.success,
        "an unresolvable token in a workflow's .js must fail the install, not \
         install as literal text: {}\n{}",
        learn.stdout, learn.stderr
    );
    let all = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        all.contains("nonesuch"),
        "the failure must name the bad referent: {all}"
    );
    assert!(
        !sb.mind_home.join("store/workflow/stray").exists(),
        "a failed install must leave no store copy"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("workflows/stray.js")).is_err(),
        "and no link"
    );
    // The failure is confined to the offending item: the rest of the source is
    // unaffected, which is what makes the hard failure safe.
    let ok = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(
        ok.success,
        "a sibling item must still install after the refusal: {}\n{}",
        ok.stdout, ok.stderr
    );
}

#[test]
fn marketplace_plugin_workflow_installs_and_links() {
    // spec: WF-40 WF-42
    // A plugin's workflows/ maps to the workflow kind and rides the normal
    // store+symlink pipeline, linking under the plugin-name prefix (MKT-5). The
    // fixture's `meta.name` is the `{{ns:}}` token, so it expands at install to
    // exactly the `<plugin>:<name>` string the harness names a plugin workflow
    // (WF-42), and `learn` prints no WF-24 divergence warning.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("workflow:acme-tools:deploy"),
        "the plugin workflow must appear in probe: {}",
        probe.stdout
    );

    let r = sb.mind(&["learn", "workflow:acme-tools:deploy"]);
    assert!(
        r.success,
        "learn workflow failed: {} {}",
        r.stdout, r.stderr
    );

    let link = sb.claude_home.join("workflows/acme-tools:deploy.js");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "the plugin workflow must be linked at workflows/acme-tools:deploy.js"
    );
    let body = std::fs::read_to_string(&link).expect("read the linked workflow");
    assert!(
        body.contains("name: 'acme-tools:deploy'"),
        "the token must expand to the harness's <plugin>:<meta.name> spelling: {body}"
    );
    assert!(
        !r.stderr.contains("the harness resolves it as"),
        "an expanded token agrees with the installed name, so no WF-24 \
         divergence warning: {}",
        r.stderr
    );
}

#[test]
fn marketplace_plugin_manifest_workflows_key_is_ignored() {
    // spec: WF-41
    // The fixture's plugin.json points `workflows` at `./elsewhere/workflows`, a
    // real directory (with its own `release.js`) that is NOT the conventional
    // `workflows/` directory. It is a component-path override mind ignores, as
    // it ignores every other one (MKT-3), so the manifest still parses and the
    // convention directory is what gets scanned -- `release` never appears,
    // because mind never looks at the declared path at all.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        r.success,
        "an unknown component-override key must not fail the meld: {} {}",
        r.stdout, r.stderr
    );

    let manifest = std::fs::read_to_string(sb.source.join(".claude-plugin/plugin.json"))
        .expect("read the fixture manifest");
    assert!(
        manifest.contains("\"workflows\""),
        "the fixture must actually declare the override key: {manifest}"
    );
    assert!(
        sb.source.join("elsewhere/workflows/release.js").is_file(),
        "the declared path must be real, so this proves mind is choosing not \
         to follow it rather than failing to find anything there"
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("workflow:acme-tools:deploy"),
        "the convention workflows/ dir is scanned, not the declared path: {}",
        probe.stdout
    );
    // Not a bare `contains("release")`: `deploy`'s own description ("Stage,
    // verify, and cut a release") already contains that substring, so this
    // checks the specific effective item key `release` would install under
    // instead (MKT-5's plugin-name prefix), plus the store, which is the
    // authoritative record of what actually got installed.
    assert!(
        !probe.stdout.contains("workflow:acme-tools:release")
            && !probe.stdout.contains("workflow:release"),
        "the workflow at the declared (but ignored) path must not appear: {}",
        probe.stdout
    );
    assert!(
        !sb.mind_home
            .join("store/workflow/acme-tools:release")
            .exists()
            && !sb.mind_home.join("store/workflow/release").exists(),
        "the ignored-path workflow must never be installed"
    );
    // The invisibility is total, not just absent from a listing: naming the
    // item outright is an unresolvable ref, and nothing links for it.
    for r#ref in ["release", "acme-tools:release", "workflow:release"] {
        let learn = sb.mind(&["learn", r#ref, "--yes"]);
        assert!(
            !learn.success,
            "`learn {ref}` must not resolve an item at the ignored path: {}\n{}",
            learn.stdout,
            learn.stderr,
            r#ref = r#ref
        );
    }
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("workflows/release.js")).is_err()
            && std::fs::symlink_metadata(sb.claude_home.join("workflows/acme-tools:release.js"))
                .is_err(),
        "nothing may link for the workflow at the declared path"
    );
}

/// The POSITIVE CONTROL for the test above: `elsewhere/workflows/release.js` is
/// a perfectly loadable workflow that mind installs the moment it is given a
/// scan root that reaches it (`--add-root`, DSC-84). Without this, the negative
/// assertions pass just as well if the fixture's `release.js` were malformed,
/// misnamed, or an empty file -- proving nothing about WF-41, since a file mind
/// COULD NOT have installed anyway is not evidence that mind declined to follow
/// the manifest's `workflows` override.
///
/// So this pins the fixture's other half: what the harness would load from the
/// declared path is a real item, and the ONLY thing keeping it out of mind is
/// that mind never reads the override.
#[test]
fn marketplace_plugin_ignored_override_path_holds_a_real_workflow() {
    // spec: WF-41 DSC-84 MKT-17
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    // --add-root composes with the plugin manifest rather than suppressing it,
    // so both workflows are offered: the conventional one and the one at the
    // path the manifest declared and mind ignored.
    let r = sb.mind(&["meld", &spec, "--register-only", "--add-root", "elsewhere"]);
    assert!(r.success, "meld --add-root: {}\n{}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("workflow:acme-tools:release"),
        "the workflow at the declared path is a real, discoverable item once a \
         scan root reaches it: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("workflow:acme-tools:deploy"),
        "and the plugin manifest's own items survive the added root (DSC-84): {}",
        probe.stdout
    );
    // Its `meta` object is readable too (WF-4), which is what makes it a
    // loadable workflow rather than a stray `.js` file.
    let by_desc = sb.mind(&["probe", "--no-tui", "declared"]);
    assert!(
        by_desc.stdout.contains("workflow:acme-tools:release"),
        "release's meta.description must be read, so the fixture is a real \
         workflow and not an unreadable file: {}",
        by_desc.stdout
    );

    let learn = sb.mind(&["learn", "workflow:acme-tools:release", "--yes"]);
    assert!(learn.success, "learn: {}\n{}", learn.stdout, learn.stderr);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("workflows/acme-tools:release.js")).is_ok(),
        "and it installs and links like any other workflow: {}",
        learn.stdout
    );
}
