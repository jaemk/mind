//! Spec-coverage gate.
//!
//! Every normative spec ID defined in `spec/*.md` (as a `- `ID` ...` list item)
//! must either be cited by a test (`// spec: ID` comments in `src/` or `tests/`)
//! or appear in the ALLOWLIST below. This fails the build when a new spec
//! requirement is added without a coverage decision, so coverage cannot silently
//! regress. See spec/README.md.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Spec IDs intentionally not cited by a dedicated test: structural invariants
/// and schema facts exercised indirectly by many tests, or secondary behaviors
/// not yet given their own test. To add a NEW spec ID, either cite it from a
/// test or add it here with a reason.
const ALLOWLIST: &[&str] = &[
    // Storage layout and JSON schema invariants, exercised by every test that
    // reads/writes the registry or manifest, or installs an item.
    "STO-1", "STO-3", "STO-10", "STO-11", "STO-12", "STO-20", "STO-21", "STO-22", "STO-23",
    "STO-30", "STO-31",
    // Lifecycle invariants covered indirectly: swap mechanics, idempotent
    // reinstall, removing an absent path. (LIFE-15, the source-content hash
    // basis, is now cited by example_drift_upgrade.)
    "LIFE-3", "LIFE-6", "LIFE-21",
    // Namespacing: install-time application and the token's written form are
    // definitional, exercised by the expansion tests.
    "NS-3", "NS-10",
    // A curated super-source adopting an un-onboarded nested source (DSC-59/60/61)
    // is implemented and cited from tests/cli.rs (the apply, gate-with-warning,
    // and consumer-pin-override tests); no longer allowlisted.
    // Retired (never implemented): INIT-8 proposed an `init-source` scaffold of a
    // `[source].install` stub; dropped when that field was deprecated (HOOK-90).
    // The statement is kept (marked removed) so the number is not reused; it has
    // no behavior and so no citing test.
    "INIT-8",
    // Tombstone: the `private = true` flag was dropped before implementation in
    // favor of the DSC-68 `on-auth-failure` inline-table form. The statement is
    // kept (marked removed) so the number is not reused; it has no behavior and
    // so no citing test.
    "DSC-67",
    // Planned features (see spec/README.md feature status = planned): documented
    // with stable IDs ahead of implementation. Each must move to a citing test
    // when built, at which point it is removed from this allowlist.
    //   unmanaged lobe items (see spec/unmanaged.md): the scan + recall + probe
    //   listing + forget (UNM-1..5) are implemented and cited from src/unmanaged.rs
    //   and tests/cli.rs; the interactive TUI group node (UNM-6) is implemented
    //   and cited from src/tui/tree.rs and src/tui/app.rs. Bulk-forget of unmanaged
    //   items via `forget --unmanaged [glob]` (UNM-7/UNM-8) is now implemented and
    //   cited from src/unmanaged.rs and tests/cli.rs; removed from the allowlist.
    //   absorb (see spec/absorb.md): claim an unmanaged lobe item into a
    //   version-controlled source and install it managed (ABS-1..10) is now
    //   implemented and cited from src/commands.rs, src/git.rs, and
    //   tests/cli_absorb.rs; no longer allowlisted. `dump` (spec/dump.md,
    //   DUMP-1..8), the nested-source `install_items` subset directive
    //   (DSC-62/63/64, discovery.md) that dump emits and the install flow honors,
    //   and the authoritative nested-entry pin (DSC-65) dump relies on are now
    //   implemented and cited from src/dump.rs, src/mindfile.rs, src/commands.rs,
    //   tests/cli_install_items.rs, tests/cli_dump.rs, and tests/cli.rs; no longer
    //   allowlisted.
    //   explicit item dependencies (see spec/dependencies.md): an optional
    //   `requires:` frontmatter key unioned with the `{{ns:}}`-derived edges
    //   (DEP-4/5/6) are now implemented and cited from src/catalog.rs, src/deps.rs,
    //   src/install.rs, src/review.rs, and tests/cli.rs. Dependency-graph operations
    //   across the verbs (DEP-60/61/62) are now implemented and cited from
    //   tests/cli.rs; no longer allowlisted. TUI dependency navigation (TUI-50/51)
    //   is now implemented and cited from src/tui/tree.rs and src/tui/app.rs;
    //   no longer allowlisted.
    //   super-source install gating + discovery (DSC-54..57, see spec/discovery.md)
    //   is implemented and cited from tests/cli.rs: the default gating (DSC-54),
    //   `meld --install-super-sources` (DSC-55), the post-meld probe hint (DSC-56),
    //   and the `sync` re-walk of the discover chain (DSC-57).
    //   version pinning: now implemented; IDs removed from allowlist and cited in tests.
    //   review verb: now implemented; IDs removed from allowlist and cited in tests.
    //   meld no-arg defaults to `.` (CLI-25, cited in tests/cli.rs) and the
    //   maintainer `init-source` scaffolder (INIT-1..6; src/namespace.rs +
    //   tests/cli.rs) are now implemented and cited; no IDs remain allowlisted.
    //   Claude plugin marketplaces (spec/marketplace.md, MKT-1..16): now
    //   implemented and cited from tests/cli.rs and unit tests in src/catalog.rs
    //   and src/mindfile.rs; no MKT IDs remain allowlisted. This includes the
    //   marketplace + curator compose (MKT-15/16): a co-present mind.toml's
    //   own-item directives (roots/flat-skills/[[items]]/[discover] globs) suppress
    //   the manifest's own-item layer, while [discover].sources composes a curator
    //   on top.
    //   self-update `evolve` verb: in-place upgrade of the mind binary using the
    //   same native curl/wget downloader as resources/install.sh (no external
    //   crate). The pure logic (platform triple, version compare/decision, the
    //   --check report) is cited from src/selfupdate.rs and tests/cli.rs
    //   (CLI-140, CLI-141). The network download (CLI-142) and the binary swap
    //   (CLI-143) need a real release and a writable install path, so they cannot
    //   run headlessly and stay allowlisted.
    "CLI-142", "CLI-143",
    //   evolve locking (STO-46) is cited from src/selfupdate.rs: the lock
    //   acquisition site carries the citation; the serialization behavior itself
    //   needs two concurrent processes and a real binary swap, so it has no
    //   dedicated headless test.
    //   install hooks (source-declared or user-supplied build command gated by a
    //   safety prompt; see spec/install-hooks.md) is fully cited: the core
    //   (parse/resolve/disclosure/run) from src/hook.rs, the data/error/parse
    //   pieces from src/source.rs, src/error.rs, src/mindfile.rs, the `review`
    //   advisory from src/review.rs, the meld/upgrade wiring (run/skip/abort,
    //   re-run gating, recording) from tests/cli.rs and src/commands.rs. No HOOK
    //   IDs remain allowlisted.
    //   enterprise managed policy (see spec/policy.md) is fully cited: the core
    //   (parse/locate/allow_matches/validate) from src/policy.rs, the enforcement
    //   (lock/pinned refusal, learn/sync/evolve gating, auto-meld provisioning,
    //   lobe lock) from tests/cli.rs and src/paths.rs, and `mind review --policy`
    //   from src/review.rs. No POL IDs remain allowlisted.
    //   within-source dependency resolution (a partial `learn` pulls in the
    //   siblings its items reference; see spec/dependencies.md) is fully cited:
    //   the resolution core (DEP-1..23, DEP-31 interaction) from tests in
    //   src/deps.rs and src/namespace.rs, the `learn` wiring (DEP-30/31/32) and
    //   the explicit non-goal (DEP-50) from tests/cli.rs, and the interactive TUI
    //   confirm-and-install of the closure (DEP-40/41) from tests in src/tui/*.rs.
    //   No DEP IDs remain allowlisted.
    //   interactive TUI: IDs with automatable logic are now cited from tests
    //   in src/tui/*.rs. Only the following remains allowlisted because it
    //   requires a real TTY to observe and cannot be verified in a headless CI:
    //     TUI-1:  interactive launch requires a physical TTY - untestable headlessly.
    //   TUI-40 (terminal restore on panic) is now cited: the poison-recovery path
    //   is exercised by a unit test in src/tui/term.rs.
    "TUI-1",
    // TUI-3: tombstone - the `-n` short form of `--no-tui` was removed (TUI-54,
    // CLI-163). The spec entry is kept so the ID is not reused; it has no
    // current behavior and so no citing test.
    "TUI-3",
    // Resource and helper tooling (spec/tooling.md) is cited: the `tool` kind and
    // discovery (TOOL-1/2/5/7) from src/catalog.rs, the path-token expander
    // (TOOL-10/11/12/14) from src/namespace.rs, and the end-to-end install
    // behavior (TOOL-3/4/6/13/15) from tests/cli.rs. Item build hooks: the
    // declaration (HOOK-70), the non-TTY skip (HOOK-72), the run-in-staging +
    // rollback (HOOK-71), the re-run on reinstall/upgrade (HOOK-73), and the
    // headless flag (HOOK-74) are all cited from src/install.rs and
    // tests/cli_build_hooks.rs.
    //   cross-harness lobes (spec/harness-lobes.md) are implemented and cited:
    //   HARN-1 (kinds filter) from src/config.rs + src/paths.rs + tests/cli_lobes.rs,
    //   HARN-2/HARN-3 (link filtering / rules Claude-only) from tests/cli_lobes.rs,
    //   HARN-4 (presets) from src/paths.rs + tests/cli_lobes.rs, HARN-5
    //   (auto-detect-and-prompt) from src/paths.rs + tests/cli_lobes.rs, and
    //   HARN-6 (verbatim link, no frontmatter rewrite) from tests/cli_lobes.rs.
    //   HARN-7 (backfill on lobe-add), HARN-8 (introspect --fix missing-lobe
    //   coverage), and HARN-9 are cited from tests/cli_lobes.rs.
    //   cross-source skill/rule/tool collision detection: NS-43 (detection) and
    //   NS-45 (non-interactive error) are now implemented and cited from
    //   src/error.rs and src/commands.rs. NS-44 (interactive TTY prompt, prompt
    //   parsing, and abort sentinel) is now cited from unit tests in
    //   src/commands.rs.
    //   rename mindfile `as` key to `namespace` in [discover].sources entries;
    //   `as` stays as a backwards-compatible alias (DSC-78): implemented and cited.
    // Namespace ergonomics: CLI-159 (--namespace flag), NS-30, and CLI-161
    // (namespace mutability lock) are now implemented and cited from tests/cli.rs.
    // TUI-53 (TUI namespace edit) is now implemented and cited from src/tui/app.rs.
    // Polished output: CLI-150 (global flags) is cited from unit tests in
    // src/main.rs; the capability gate (CLI-151), glyph/color semantics and the
    // ASCII fallback (CLI-152), the structured JSON result for mutating verbs
    // (CLI-153), and the NO_COLOR/non-UTF-8/--ascii gate-off conditions (CLI-154)
    // are now cited from integration tests in tests/cli.rs. The rich (TTY) branch
    // of the gate is unit-tested in src/render.rs (it needs a real PTY headlessly).
    // CLI-162 (--verbose global flag) is cited from unit tests in src/main.rs
    // and integration tests in tests/cli.rs.
    //   `mind hooks run` / `hooks list` (HOOK-100..104, CLI-194..196) are now
    //   implemented and cited from tests/cli_hooks.rs and unit tests in
    //   src/main.rs, src/resolve.rs, and src/hooks_cmd.rs.
];

#[test]
fn every_spec_id_is_cited_or_allowlisted() {
    let defined = defined_ids();
    assert!(
        defined.len() > 50,
        "found only {} spec IDs; the parser or spec layout likely changed",
        defined.len()
    );
    let cited = cited_ids();
    let allow: BTreeSet<String> = ALLOWLIST.iter().map(|s| s.to_string()).collect();

    // Every ID a test cites must be defined in the spec (catches typos and
    // behavior added without a spec entry).
    let undefined: Vec<_> = cited.difference(&defined).cloned().collect();
    assert!(
        undefined.is_empty(),
        "tests cite spec IDs not defined in spec/ (document them): {undefined:?}"
    );

    // The allowlist must not rot: every entry must be a real defined ID.
    let stale: Vec<_> = allow.difference(&defined).cloned().collect();
    assert!(
        stale.is_empty(),
        "ALLOWLIST references unknown spec IDs: {stale:?}"
    );

    // Keep the allowlist tight: a now-cited ID should be removed from it.
    let redundant: Vec<_> = allow.intersection(&cited).cloned().collect();
    assert!(
        redundant.is_empty(),
        "these IDs are now cited by tests; remove them from ALLOWLIST: {redundant:?}"
    );

    let uncovered: Vec<_> = defined
        .iter()
        .filter(|id| !cited.contains(*id) && !allow.contains(*id))
        .cloned()
        .collect();
    assert!(
        uncovered.is_empty(),
        "spec IDs with no test citation (add a test that cites them, or ALLOWLIST them): {uncovered:?}"
    );
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// True for tokens shaped like a spec ID: 2-4 uppercase letters, `-`, digits.
fn is_id(tok: &str) -> bool {
    match tok.split_once('-') {
        Some((alpha, num)) => {
            (2..=4).contains(&alpha.len())
                && alpha.bytes().all(|b| b.is_ascii_uppercase())
                && !num.is_empty()
                && num.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// IDs defined in the spec: the backticked token leading a `- ` list item.
fn defined_ids() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for md in files_with_ext(&root().join("spec"), "md") {
        let text = std::fs::read_to_string(&md).unwrap();
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("- `")
                && let Some(end) = rest.find('`')
            {
                let tok = &rest[..end];
                if is_id(tok) {
                    out.insert(tok.to_string());
                }
            }
        }
    }
    out
}

/// IDs cited in `src/` and `tests/` via `// spec:` comments, excluding this file.
/// Only text after a `// spec:` marker is scanned, so incidental tokens like
/// "UTF-8" in prose are not mistaken for IDs.
fn cited_ids() -> BTreeSet<String> {
    const MARKER: &str = "// spec:";
    let mut out = BTreeSet::new();
    let mut sources = files_with_ext(&root().join("src"), "rs");
    sources.extend(files_with_ext(&root().join("tests"), "rs"));
    for f in sources {
        if f.file_name().is_some_and(|n| n == "spec_coverage.rs") {
            continue; // don't count the ALLOWLIST literals as citations
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for line in text.lines() {
            if let Some(idx) = line.find(MARKER) {
                for tok in id_tokens(&line[idx + MARKER.len()..]) {
                    out.insert(tok);
                }
            }
        }
    }
    out
}

/// Extract maximal `[A-Za-z0-9-]` runs that look like spec IDs.
fn id_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            cur.push(c);
        } else {
            if is_id(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    if is_id(&cur) {
        out.push(cur);
    }
    out
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_with_ext(&path, ext));
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
    out
}
