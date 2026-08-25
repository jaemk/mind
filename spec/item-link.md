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
  curate individual skills. A URL with a `tree`/`blob` segment whose tail fails
  to complete as a valid item link is `BadItemLink` (LNK-14), not the generic
  `InvalidRepoSpec`. For a remote URL (the `scheme://host/owner/repo/...`
  branch), this holds only once the `owner/repo` portion ahead of the marker
  itself parses: a malformed `owner/repo` there still reports
  `InvalidRepoSpec` regardless of the marker or the tail, since owner/repo
  splitting runs before the tail is parsed. The local `file://` branch parses
  in the opposite order -- the `<ref>/<path>` tail is parsed before the
  `owner/repo` portion is sliced off the remaining path -- so with no usable
  repo path at all ahead of the marker (e.g. `file:///tree/main`), a broken
  tail still reports `BadItemLink` rather than falling through to
  `InvalidRepoSpec`. This is a real behavioral difference between the two
  branches, not just wording; pinned by
  `file_link_bad_tail_with_no_repo_ahead_of_marker_is_still_bad_item_link` and
  `github_malformed_owner_repo_wins_over_bad_tail` in `src/source.rs`.
- `LNK-3` The `<ref>` segment supplies the instance's pin: a 40-hex ref is a
  commit pin (as `--pin <sha>`), anything else follows that branch (as
  `--pin branch=<name>`). An explicit consumer pin flag (CLI-17) overrides the
  URL's ref; the URL's ref overrides the repo's `[source]` pin directive
  (DSC-41). `learn <url> --pin` (a bare flag on `learn`, CLI-200) freezes the link's
  branch ref to its current commit, so a branch-ref link can be snapshotted in one
  step. The ref is a single path segment: a branch name containing `/` is not
  representable in a link (the first segment after `tree`/`blob` is taken as the
  ref); meld the repo with `--pin branch=<name>` instead.
- `LNK-14` A spec whose path carries a `tree`/`blob` link marker (LNK-1) is
  unambiguously an attempted item link, so once the `owner/repo` portion ahead
  of the marker parses, every failure in its `<ref>/<path>` tail reports
  `BadItemLink`, never the generic `InvalidRepoSpec`: a missing ref, a missing
  skill path, a `blob` link not ending in `/SKILL.md`, and an unsafe path or
  ref value (LNK-10) all take this branch. The message names the offending URL
  and the two expected shapes, `<repo-url>/tree/<ref>/<skill-dir>` and
  `<repo-url>/blob/<ref>/<skill-dir>/SKILL.md` (plus the GitLab `/-/tree/` and
  `/-/blob/` spellings), instead of the generic repo-spec message, so a user
  who pasted a real forge URL is told what shape a link needs rather than that
  the URL is invalid. A spec with no `tree`/`blob` marker at all is not an
  attempted link and keeps reporting `InvalidRepoSpec` unchanged.

## Identity and lifecycle

- `LNK-4` An item-link source's identity is `host/owner/repo#<path>`, where
  `<path>` is the skill directory's repo-root-relative path. An identity alias
  composes as a trailing `@<alias>` segment (STO-58): `host/owner/repo#<path>@<alias>`.
  Instances from the same repo, and a plain meld of `host/owner/repo`, are
  distinct registry entries with independent pins, commits, and lifecycles.
  Re-melding an identical link follows the CLI-12 re-meld flow. The `#<path>` IS
  part of the clone leaf (STO-70: an item-link instance's leaf encodes its
  `item_path`, not just its `@<alias>`), so every link instance -- aliased or
  not -- gets its own independent checkout, distinct from a plain meld of the
  repo and from every other link into it. Only a genuinely bare instance (no
  `item_path`, no `@<alias>`) can ever share the plain `sources/<host>/<owner>/<repo>`
  clone, and by construction there is at most one such instance per identity
  (a second bare meld of the same repo is a re-meld, CLI-12, not a second
  instance), so in practice nothing shares a checkout post-STO-70. (Before
  STO-70, the `#<path>` was NOT part of the leaf, so every non-aliased link
  into a repo, and a plain meld of it, all shared one clone; two non-aliased
  links pinned to different refs would contend over that one working tree,
  each re-clone silently discarding the others' checkout. STO-70's migration
  note applies to any instance registered under that older behavior.)
- `LNK-5` `sync`, `upgrade`, `introspect`, `recall`, and `unmeld` treat the
  instance as an ordinary source: `sync` fetches per its pin (CLI-55),
  `upgrade` compares source content hash and commit, `unmeld` uninstalls its
  item (CLI-21). `forget` of the instance's skill leaves the instance
  registered; when the instance has no installed items left, `forget` prints a
  hint to `mind unmeld <identity>`.
- `LNK-15` The identity-alias suffix (STO-58) distinguishes link instances of
  the SAME `<path>` exactly as it distinguishes any other source instance: a
  bare link `host/owner/repo#<path>` and an aliased link
  `host/owner/repo#<path>@<alias>` are distinct registry entries that coexist,
  each with its own clone (STO-70: the `item_path` alone is now enough to earn
  an independent checkout, so both the bare and the aliased instance here get
  one), pin, and installed items, untouched by the other's lifecycle.
  Consequently `learn <url>`
  (LNK-6), with or without `--pin`, checks membership under the SPECIFIC
  alias it is melding as (here, none): if the repo has only been melded under
  a DIFFERENT alias (or no alias at all when the new call supplies one), the
  bare identity is not yet registered, so `learn` registers it as a new,
  coexisting instance rather than reusing or being blocked by the
  differently-aliased one. This is not a collision: it is the normal STO-58
  per-instance model applied to links.
- `LNK-16` A link's `<path>` may not contain `@` or `#`; a link URL whose skill
  path carries either is a `BadItemLink`. This is STO-64's rule one segment
  over. A link instance's identity is `host/owner/repo#<path>`, and an identity
  alias appends `@<alias>` to the whole identity (STO-58), so an unaliased link
  to `skills/foo@bar` and an `@bar`-aliased link to `skills/foo` would compute
  the same identity and, per STO-59, the same clone directory; a `#` would put a
  second marker into an identity that is split on the first one. Refusing both
  at parse time keeps a link identity decomposable back into the parts it was
  built from. Note that `#` reaches the path only through the `file://` form: a
  remote URL is truncated at its first `#` as a pasted browser fragment (LNK-1)
  before the path is taken, so `.../skills/foo#bar` links to `skills/foo`.
- `LNK-17` The `<path>` is lexically normalized before it becomes identity:
  `.` segments are dropped, so `.../tree/main/./skills/foo` and
  `.../tree/main/skills/./foo` parse to the same `skills/foo` instance
  (identity, clone directory, and re-meld detection) as the plain form,
  rather than registering a duplicate instance of the same on-disk skill.

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
- `LNK-18` A link instance's catalog is exactly the linked skill (LNK-7), so an
  intra-source reference (dependencies.md, tooling.md) to any other name can
  never resolve, however the repo is laid out. The two reference FORMS are
  handled differently, following DEP-4's own distinction between them:
  - A `requires:` entry (DEP-4) that names something other than the linked skill
    is NOT the DEP-6 `BadReference` error here. It is dropped, recorded
    (LNK-19), and warned about, and the skill installs. `requires` is pure
    metadata (DEP-4): it is never rewritten into the item body, so a dangling
    entry degrades the item rather than corrupting it, and refusing would make a
    skill that declares one unreachable by link at all. Other `requires`
    failures keep their DEP-7 causes as hard errors: a malformed ref
    (`InvalidRef`) and a source-qualified ref (`CrossSource`) are wrong
    regardless of the catalog. `AmbiguousKind` cannot arise, since a single-item
    catalog offers at most one match. An entry naming the linked skill's own
    name under the WRONG kind (`agent:<skill-name>`) is a `NoMatch` like any
    other and is dropped with the rest.
  - A TOKEN naming a sibling stays a hard error and nothing is installed: a
    token IS rewritten into the item's text at install, so it cannot be left
    dangling. This covers every token family that resolves against siblings, not
    just `{{ns:name}}` (NS-10/DEP-1): `{{tools:name}}` (TOOL-15) and
    `{{path:[kind:]name}}` (TOOL-18) are equally unsatisfiable in a one-item
    catalog, and a skill shipping alongside a sibling tool is a common shape.
    `{{self}}` is not a sibling reference and always resolves. The scan covers
    exactly the files install expands, markdown (NS-53) plus any non-markdown
    file the item lists in `expand:` (NS-57); scanning a narrower set would let a
    reference slip through to the blunt error this rule exists to replace. The
    error is `LinkRefUnsatisfiable`, not the generic `BadReference`/TOOL-17
    cause: it names the token as written, says the source is a single-skill item
    link with no siblings, and carries the remedy, so a user who pasted a deep
    URL is told what to run instead of being told the skill references something
    missing.

  *The remedy.* Both paths name one of two forms of the same two-step command,
  decided by scanning the clone already on disk as an ordinary whole-repo
  source (the same `Source`, with `item_path` cleared). When that scan finds a
  `Skill` at the exact path the link points at (a same-named skill declared at
  a DIFFERENT path in the repo does not count), a plain meld would discover it
  on its own:

  ```
  mind unmeld '<identity>' --yes && mind meld '<repo-url>' --learn 'skill:<skill>' --yes
  ```

  Otherwise -- an authoritative `mind.toml` or `.claude-plugin` manifest that
  does not declare the item, the case item links exist for -- a plain meld
  would not reach it, so the remedy adds a scan root:

  ```
  mind unmeld '<identity>' --yes && mind meld '<repo-url>' --add-root '<root>' --learn 'skill:<skill>' --yes
  ```

  (CLI-236, DSC-84). Either form drops the link instance and then melds the
  whole repo, installing just that skill together with its dependency closure
  (DEP-30). Any error from the reachability scan -- an unreadable clone, a
  malformed `mind.toml`, a version gate -- answers "not reachable" rather than
  propagating, so the remedy falls back to the added-root form; the scan
  runs only when a remedy is about to be printed, not on every link install.

  The `<root>` is DERIVED from the link's own path, not fixed at `.`. An added
  root is convention-scanned one level deep (flat child directories, plus a
  `skills/` container directly under it), so the root that reaches the linked
  skill is its parent directory, or its grandparent when that parent is the
  `skills/` container: `skills/foo` and a flat `foo` both give `.`, while
  `vendor/pkg/skills/foo` gives `vendor/pkg` and a flat `vendor/foo` gives
  `vendor`. A fixed `.` would reach only a skill at the repo root, so for
  anything deeper the meld half would fail with `LearnPatternNoMatch` AFTER the
  unmeld half had already succeeded -- the destroy-then-fail sequence this
  two-branch remedy exists to prevent.

  The `unmeld` step is required. The link instance is registered before the
  install runs, so it is registered on both paths when this is printed -- the
  warning path goes on to install the skill, the error path installs nothing
  -- and a bare `meld ... --learn` would therefore leave a second source for
  the same repo, and collide with the name the link already installed on the
  warning path (NS-43/CLI-33). Removing the instance first is what makes the
  printed command work when pasted.

  The `<identity>` is the instance's registered identity (LNK-4) and the
  `<repo-url>` its recorded clone URL, never the deep link URL. The pattern is
  kind-qualified (`skill:<name>`): an item link is always a skill (LNK-7), and
  a bare name would additionally match a same-named agent or rule in the repo,
  installing prompt content the user never asked for. Each of the three values
  is sanitized (DSC-95) and shell-quoted (CLI-225) before composition, and the
  identity and the name are also glob-escaped: `--learn`'s pattern and
  `unmeld`'s selector (CLI-28) both read `*`, `?`, and `[` as glob syntax, and
  `is_safe_item_name` (DSC-71/DSC-96) permits those characters. A skill named
  `pdf[x]` would otherwise make the pattern match some OTHER item, and --
  since a link identity embeds the skill's repo path (LNK-4) -- would make the
  raw identity compile as a pattern matching no source, stopping the remedy at
  its first command. Escaping is a no-op for an identity or name with no
  metacharacters, so both commands above are unchanged for the common case.

  *Where it applies.* Wherever a link instance's skill is installed: `learn
  <url>` (LNK-6), the install pass of `meld <url>`, and a later `upgrade` of the
  instance, so an upstream edit that adds a `requires` entry does not turn every
  subsequent upgrade of that link into a hard failure.

  *What it cannot fix.* One shape defeats the added-root form: a declared
  inventory that offers a DIFFERENT item of the same bare name (say it declares
  `vendor/review` while the link points at `skills/review`). The reachability
  scan correctly answers "not reachable" there, since it matches by path, but
  the added root then makes both items visible at once and the meld half fails
  with the DSC-85 `DuplicateItem` guard ("appears under more than one scan
  root"). The unmeld half has already run at that point, so this is the one
  remaining case where the remedy leaves the user worse off than before pasting
  it. Reaching that skill needs `--root` (replacing the inventory's roots)
  rather than `--add-root` (composing with them), which is a choice about what
  the whole source offers, not something a per-item remedy should make.

  The added-root form otherwise reaches a skill an authoritative
  `mind.toml` or `.claude-plugin` manifest does not declare (DSC-84) at any
  depth, since the root is derived from the link's path. What it does not carry
  over is the rest of the source's consumer configuration: the remedy melds the
  repo fresh, so a `--namespace`, `--pin`, or `--flat-skills` the original link
  was melded with is not reproduced, and neither are any `--add-root` values
  beyond the one derived here. A link melded with extra scan roots therefore
  needs them restated by hand on the meld half.

- `LNK-19` A `requires:` entry dropped under LNK-18 is recorded on the installed
  item (`dropped_requires` in the manifest) rather than existing only as the
  install-time warning. The warning scrolls away, is reduced to a summary line by
  the TUI's output capture, and goes to stderr under `--json`, so without a
  durable record an item can carry a silently absent dependency with no way to
  discover it later. The record is what `recall <item>` shows as a `dropped`
  line, what `recall <item> --json` carries as a `dropped_requires` array (absent
  when empty), and what `introspect` reports as a `dropped-requires` issue naming
  the item and the entry. `introspect --fix` does not repair it: the only fix is
  to replace the link with the whole repo, which changes which sources are
  registered and so is the user's call. The field is re-derived on every install
  and upgrade of the item, so it always describes the version on disk; it is
  empty (and omitted from `manifest.json`) for every ordinary install, so an item
  installed by an older binary, or from a non-link source, reads back unchanged.

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

## Reproduction

- `LNK-13` `dump` emits a link instance as a `[discover].sources` entry: its
  `source` is a `tree/<ref>/<path>` deep URL reconstructed from the recorded
  `host`/`owner`/`repo` (a `file://`-prefixed path for a local instance,
  matching LNK-1's local form) and `item_path`, with the recorded commit as
  the `<ref>` segment. The entry also carries `pin-ref = <commit>` (dump.md
  DUMP-1/DUMP-4), the SAME commit as the URL's own ref: `pin-ref` is what
  actually pins the reproduction (a curator pin outranks an item link's own
  URL-ref pin, DSC-65 over LNK-3), and the URL's ref exists to keep the URL
  syntactically complete (a `tree`/`blob` link always needs one) and in
  agreement with `pin-ref` rather than a possibly-since-moved branch tip. The
  entry's `namespace` carries the recorded identity alias (`as_alias`,
  STO-58), not a display-only prefix, so re-melding reproduces the exact
  `host/owner/repo#path@alias` instance (LNK-4) an aliased link was
  registered under; an unaliased link emits no `namespace` and, on re-meld,
  naturally re-reads the repo's own `[source].prefix` (if any) exactly as the
  original meld did. `roots`/`add-roots`/`flat-skills` are never emitted for
  a link entry: a link instance's catalog is exactly its one skill (LNK-7),
  so convention scan roots do not apply to it. A link instance always has a
  recorded commit once registered (LNK-3/LNK-4: its checkout point is never
  the default branch, so `meld`/`learn`/`sync` always resolve and record one
  before it is added to the registry); the only way `dump` cannot reconstruct
  the entry is a hand-edited registry with a missing commit, in which case
  `dump` skips the instance with a note rather than emit an unpinned entry.
  The reconstructed URL is always the `https://` form built from the recorded
  `host`/`owner`/`repo`, never the source's own `url` field: `prefer_ssh`
  (DSC-66) may have rewritten `url` to the `git@host:owner/repo` SSH form
  after the meld, and that form cannot express a `tree/<ref>/<path>` link
  suffix, so `dump` always derives the link URL from the identity parts
  rather than echoing `url` verbatim.
