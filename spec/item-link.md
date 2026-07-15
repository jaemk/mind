# Item links (single-item source instances)

How `mind` consumes a deep link to one skill inside a repo - the URL a user
copies from their forge's file browser - as a self-contained, managed install.
A repo may publish a `.claude-plugin/marketplace.json` that lists only a subset
of its skills (marketplace.md); an item link reaches a skill the manifest does
not list, because the consumer names the exact path.

Each accepted link becomes its own *source instance*: a registry entry with an
extended identity, its own clone, pin, and lifecycle. Several links into the
same repo, and a plain meld of that repo, coexist as separate sources.

## Link form

- `LNK-1` An item link is a URL naming a directory or `SKILL.md` inside a
  repo: `https://<host>/<owner>/<repo>/tree/<ref>/<path>` or
  `https://<host>/<owner>/<repo>/blob/<ref>/<path>/SKILL.md`, plus the GitLab
  `/-/tree/<ref>/<path>` and `/-/blob/<ref>/<path>/SKILL.md` variants, and the
  `file:///<repo-path>/tree|blob/...` form for a local repo (the marker is
  only recognized in the explicit `file://` spelling, so a bare local path
  containing a `tree` directory stays a plain repo spec). A `blob` link must
  end in `/SKILL.md` and names the skill directory's anchor file; the skill
  directory is its parent. A `tree` link names the skill directory itself (a
  trailing `/SKILL.md` is also accepted). A query string or fragment
  (`?plain=1`, `#L10`) is stripped before parsing. Item links apply to the
  skill kind only (the `SKILL.md` anchor is what makes the target
  classifiable); a link to an agent, rule, or tool path is not recognized. A
  local `file://` link instance is always a cloned snapshot at its pin (the
  CLI-27 pinned-local flow), never a live-read working tree.
- `LNK-2` An item link is accepted anywhere a repo spec (CLI-11) is: `meld`,
  `learn` (LNK-6), and a `[discover].sources` entry (DSC-38), so a curator can
  curate individual skills. A URL with a `tree`/`blob` segment that does not
  parse as an item link is `InvalidRepoSpec`.
- `LNK-3` The `<ref>` segment supplies the instance's pin: a 40-hex ref is a
  commit pin (as `--pin-ref`), anything else follows that branch (as
  `--follow-branch`). An explicit consumer pin flag (CLI-17) overrides the
  URL's ref; the URL's ref overrides the repo's `[source]` pin directive
  (DSC-41). The ref is a single path segment: a branch name containing `/` is
  not representable in a link (the first segment after `tree`/`blob` is taken
  as the ref); meld the repo with `--follow-branch` instead.

## Identity and lifecycle

- `LNK-4` An item-link source's identity is `host/owner/repo#<path>`, where
  `<path>` is the skill directory's repo-root-relative path. Its clone lives at
  `sources/<identity>` (STO-11 extended). Instances from the same repo, and a
  plain meld of `host/owner/repo`, are distinct registry entries with
  independent clones, pins, commits, and lifecycles. Re-melding an identical
  link follows the CLI-12 re-meld flow.
- `LNK-5` `sync`, `upgrade`, `introspect`, `recall`, and `unmeld` treat the
  instance as an ordinary source: `sync` fetches per its pin (CLI-55),
  `upgrade` compares source content hash and commit, `unmeld` uninstalls its
  item (CLI-21). `forget` of the instance's skill leaves the instance
  registered; when the instance has no installed items left, `forget` prints a
  hint to `mind unmeld <identity>`.

## Install

- `LNK-6` `learn <url>` is the one-shot form: it registers the link instance
  exactly as `meld <url>` would (clone, hook consent, prompts) and installs its
  skill in one step. `meld <url>` follows the standard meld flow (the CLI-23
  install offer, `--register-only`, `--yes`). The meld flags apply to a link
  meld unchanged (`--namespace`, pin flags, `--install-hook`).
- `LNK-7` A link instance's catalog is exactly one skill: the directory at
  `<path>` containing `SKILL.md`, bare name the directory's basename,
  description from frontmatter (DSC-30). Discovery bypasses the repo's declared
  inventory: an authoritative `mind.toml` (DSC-3) and a `.claude-plugin/`
  manifest (MKT-2/MKT-14) do not gate the link, so it can install a skill the
  repo does not export - the consumer named the exact path. The
  `min-mind-version` gate (DSC-40) and `[source]` metadata (description,
  declared prefix) still apply. A `<path>` whose clone content has no
  `SKILL.md` is an error (`LinkNotASkill`) and nothing is registered.
- `LNK-8` A link instance registers no nested sources: `[discover].sources`
  entries and marketplace external plugins in the linked repo are not walked.
  The link is a single-item grab, not a super-source adoption.
- `LNK-9` Namespacing and collisions are unchanged: the effective prefix is
  the alias or `[source].namespace` (NS rules), and collisions with installed
  items surface through the existing checks (NS-41, NS-43, CLI-33). Source
  install hooks are disclosed and consent-gated as at any meld (HOOK-20,
  HOOK-50..60).

## Safety and policy

- `LNK-10` The `<path>` must be a safe relative path (the DSC-71..73 rule: not
  absolute, not `~`-rooted, no `..` component, no NUL); the `<ref>` is
  validated as a git ref value (DSC-66). A violation is `InvalidRepoSpec` at
  parse, before any clone.
- `LNK-11` Managed-policy allowlist matching (POL-11, POL-36) uses the base
  repo identity `host/owner/repo`, not the extended instance identity: a
  policy that allows the repo allows links into it. `require-pinned` (POL-20)
  evaluates the instance's effective pin as usual, so a branch-ref link is
  refused under a pinned policy.

## Display

- `LNK-12` `recall --sources` and the probe source view show a link instance
  under its full `#`-suffixed identity. Compare and browse URLs (CLI-176,
  HOOK-24) derive from the base repo and keep their host-gated shapes.

## Planned

- `LNK-13` (planned) `dump` emits a link instance as a `[discover].sources`
  entry whose `source` is a deep URL reconstructed from the base URL, the
  recorded pin, and the item path, so a dumped super-source reproduces link
  installs. Until implemented, `dump` skips link instances with a note.
