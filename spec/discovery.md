# Discovery

How a source's installable items are found and described. The catalog is source
truth: it holds bare names; prefixing and token expansion are install-time
transforms (see namespacing.md).

## Precedence

Three layers, in order:

- `DSC-1` Convention discovery is the zero-config default: any melded repo is
  scanned with no manifest required.
- `DSC-2` Per-item frontmatter is always read for an item's description.
- `DSC-3` A `mind.toml` at the repo root is optional. `[source]` metadata is read
  whenever the file is present. If it declares `[[items]]` or `[discover]` item
  globs it becomes authoritative and convention discovery is skipped for that
  source. A bare `[discover].sources` list (no item globs) does not disable
  convention.

A fourth, external-manifest layer reads Claude Code's native plugin manifests
(`.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`) as a discovery
input for repos published to the built-in plugin system. It feeds the same catalog
and leaves the install model unchanged; an authoritative `mind.toml` (DSC-3) still
wins over it. A co-present `mind.toml` composes at a finer grain: own-item
directives (`[[items]]`, `[discover]` item globs, `roots`, `flat-skills`) suppress
the manifest's own-item layer, while a `[discover].sources` list layers a curated
super-source on top of the manifest (MKT-15, MKT-16). See
[marketplace.md](marketplace.md) (MKT-1..16).

## Convention

- `DSC-10` A skill is a directory `skills/<name>/` containing `SKILL.md`; its name
  is the directory name.
- `DSC-11` An agent is a file `agents/<name>.md`; its name is the file stem.
- `DSC-12` A rule is a file `rules/<name>.md`; its name is the file stem.
- `DSC-13` A missing `skills/`, `agents/`, or `rules/` directory yields no items
  (not an error).

## Frontmatter

- `DSC-20` An item's description is the top-level `description` from the YAML
  frontmatter of its `SKILL.md` (skill) or its `.md` (agent, rule).
- `DSC-21` The frontmatter reader handles a leading `--- ... ---` block with
  top-level scalar keys and surrounding quotes, including block scalars (DSC-22).
  Flow collections and nested mappings are not interpreted. No frontmatter yields
  no description.
- `DSC-22` The frontmatter reader interprets a top-level block scalar value: a
  folded scalar (`>`, `>-`, `>+`) or a literal scalar (`|`, `|-`, `|+`) introduced
  by the indicator after the colon. The value is the run of more-indented lines
  that follow, ending at the first line that dedents to or below the key's
  indentation or at the closing `---`. A folded scalar joins its lines with single
  spaces (a blank line is a paragraph break); a literal scalar preserves its line
  breaks; the chomping indicator (`-` strip, `+` keep, none clip) governs trailing
  newlines. The result is trimmed for display in `recall`/`probe`. Other nested
  structures and flow collections remain uninterpreted.
- `DSC-23` The frontmatter reader strips a leading UTF-8 BOM (`\xEF\xBB\xBF`,
  U+FEFF) from the file text before checking for the `---` opening delimiter.
  A file authored on Windows or by an editor that emits a BOM still has its
  frontmatter read correctly.

## mind.toml

```toml
[source]
description = "..."          # optional; shown by recall --sources
prefix = "jk"                # optional; namespace (see namespacing.md)
min-mind-version = "0.2"     # minimum mind version; enforced at scan/meld (DSC-40)
roots = ["packages/tools"]   # optional; convention scan roots (DSC-50)
flat-skills = true           # optional; skills are bare dirs at a root, no
                             #   skills/ container (DSC-74)
follow-branch = "main"       # optional; pin directive, one of
                             #   follow-branch / pin-tag / pin-ref (DSC-41)

[[items]]                     # explicit inventory (authoritative)
kind = "skill"               # skill | agent | rule | tool
name = "review"
path = "skills/review"       # relative to repo root; a dir for skills
link = "rules/x.md"          # optional; link target relative to ~/.claude
description = "..."          # optional; overrides frontmatter

[discover]                    # glob discovery (authoritative for items)
skills = { include = ["packages/*/SKILL.md"], exclude = ["packages/internal-*/SKILL.md"] }
agents = { include = ["agents/*.md"] }
rules  = { include = ["rules/*.md"] }
# A curated super-source: list other repos to meld recursively.
sources = [
  { source = "owner/repo" },
  { source = "github:foo/bar", as = "fb" },        # impose a namespace on the nested source
  { source = "owner/recommended", install = true } # offer this one for install on meld
]
# Adopt an un-onboarded source: supply config it lacks (applied only when it has
# no mind.toml of its own). The array-of-tables form carries hooks:
[[discover.sources]]
source = "owner/unonboarded"
on-auth-failure = { action = "skip", message = "..." } # optional; skip if auth fails (DSC-68)
follow-branch = "main"           # pin directive for the nested source (DSC-41)
roots = ["packages/agents"]      # scan roots for the nested source (DSC-50)
flat-skills = true               # flat skill layout for the nested source (DSC-77)
[[discover.sources.hooks]]       # build hooks for the nested source (HOOK-50)
run = "make build"
```

- `DSC-30` Unknown top-level or table fields are rejected (the file is strict).
- `DSC-31` A `[[items]]` entry with an unknown `kind` is an error (`MindToml`).
- `DSC-71` A `[[items]]` `name` must be a single safe path component: it is
  rejected (`MindToml`) when empty, equal to `.` or `..`, or containing a path
  separator (`/` or `\`) or a NUL byte. The name indexes the store
  (`store/<kind>/<name>`) and the per-home link (`<home>/<dir>/<name>`), so this
  stops a melded source from steering either path outside its kind directory.
  (Convention and `[discover]`-glob names are derived from a single filesystem
  component, so they cannot carry a separator and are inherently safe.)
- `DSC-72` A `[[items]]` `link` override (the link target relative to an agent
  home) must be a safe relative path: it is rejected (`MindToml`) when empty,
  absolute, beginning with `~`, containing a `..` (parent) component, or
  containing a NUL byte. So a melded source cannot place a symlink outside the
  agent home (e.g. `link = "../../.bashrc"`).
- `DSC-73` A `[[items]]` `path` must be a safe repo-root-relative path: it is
  rejected (`MindToml`) when empty, absolute, beginning with `~`, containing a
  `..` (parent) component, or containing a NUL byte. A relative value free of
  those components may contain `/` for subdirectories (e.g.
  `guidelines/style.md`). This stops a melded source from reading host files
  outside its clone via the copy-to-store step (e.g.
  `path = "../../../../etc/passwd"` or `path = "/home/victim/.ssh/id_rsa"`).
  The same safety rule (`is_safe_link_rel`) that validates `link` (DSC-72)
  applies to `path`.
- `DSC-32` An item's description is its `mind.toml` `description` if given, else
  its frontmatter description.
- `DSC-33` Each `[discover]` kind (`skills`, `agents`, `rules`) is a table with
  `include` and optional `exclude` glob lists, relative to the repo root. A skill
  glob matches a `SKILL.md` (the item is its parent directory); agent and rule
  globs match the `.md` file directly.
- `DSC-34` `[[items]]` and `[discover]` may both appear; their results are unioned.
- `DSC-35` A source with only `[source]` metadata, or only `[discover].sources`
  (no item globs), still uses convention discovery for its own items.
- `DSC-36` A repo with no `mind.toml` is unaffected by all of the above.
- `DSC-37` Within a kind, `include` globs are matched first, then any path also
  matched by an `exclude` glob is dropped from the result.
- `DSC-38` `[discover].sources` lists other repo specs (each parsed like a `meld`
  argument). Melding a source recursively melds each listed source, skipping any
  already registered, so a `mind.toml` can act as a curated registry /
  super-source. Each curated source is registered independently and tracks its
  own upstream commit. Recursion always terminates, even when a nested source is
  itself a super-source and the chain forms a cycle (`A -> B -> C -> A`): a source
  is registered before its own nested sources are processed, and a source already
  seen is skipped, matched both by URL within the run and by `host/owner/repo`
  identity against the registry (so two spellings of the same repo do not slip
  past). Each source in the transitive set is therefore processed at most once.
- `DSC-39` A `[discover].sources` entry may set `as = "<prefix>"` to impose a
  namespace on that nested source (equivalent to `meld --as`).
- `DSC-58` A `[discover].sources` entry may set `install = true` (default false)
  to recommend that nested source for install: melding the super-source offers its
  items via the same preview-and-prompt as the top-level source (CLI-23, honoring
  `--yes` and skipped under `--link-only`), rather than leaving them only
  registered and available (DSC-54). The flag is per entry, so a curator chooses
  which nested sources install by default and which stay available. It applies on
  a fresh meld and a re-meld, and the whole chain is still traversed so a deeper
  `install = true` is reached even under an unflagged parent. `meld --recursive`
  (DSC-55) is the superset: it installs every nested source regardless of the flag.
- `DSC-62` A `[discover].sources` entry may set `install-items = ["skill:name",
  ...]`, a list of bare `kind:name` refs that scopes install to exactly those of
  the nested source's items, instead of `install = true` (offer all, DSC-58) or
  `install = false` (offer none). When the super-source is melded and the entry is
  reached by the install flow (DSC-54, DSC-55), only the listed items are offered
  via the same preview-and-prompt as the top-level source (CLI-23, honoring
  `--yes`, skipped under `--link-only`); the source's other items are registered
  and left available. `install-items = []` is equivalent to `install = false`.
  When `install-items` is present it governs the entry's install behavior; when it
  is absent the `install` boolean governs as before (DSC-58).
- `DSC-63` Refs in `install-items` (DSC-62) are bare `kind:name` in source truth;
  a prefix in effect for the entry (`as`, DSC-39) is applied at install. A ref
  naming an item the nested source does not offer is an error (`BadReference`) at
  meld, not a silent skip.
- `DSC-64` Setting `install = true` together with a non-empty `install-items` on
  the same entry is a `MindToml` error: offering all and offering a named subset
  are mutually exclusive. The subset form is expressed by `install-items` alone
  (with `install` left unset or false).
- `DSC-59` A `[discover].sources` entry may carry configuration for the
  nested source that the source itself would normally declare in its own
  `mind.toml`: a pin directive (exactly one of `follow-branch = "<branch>"`,
  `pin-tag = "<tag>"`, or `pin-ref = "<commit>"`, per DSC-41), `roots = [...]`
  (convention scan roots, DSC-50), `flat-skills = true` (flat skill layout,
  DSC-74; see DSC-77), and one or more hooks as a
  `[[discover.sources.hooks]]` array-of-tables (the `[[hooks]]` shape, HOOK-50).
  Declaring more than one pin directive on an entry is a `MindToml` error, the
  same one-of rule a `[source]` section follows (DSC-41). This lets a curator add
  support for a source that has not onboarded itself (no `mind.toml`), including
  one with custom build requirements or a monorepo layout, without forking it,
  and lets a generated super-source pin each source to a reproducible revision
  (DSC-65, DUMP-1).
- `DSC-60` The curator-supplied `roots`, `flat-skills`, and `hooks` (DSC-59)
  apply only when the nested source ships no `mind.toml` of its own. When the
  nested source has a `mind.toml`, that file is authoritative for its roots,
  flat-skills, and hooks and the curator-supplied values are ignored (a warning is
  emitted, since the source has onboarded). The gate is whole-file: a nested
  `mind.toml`, even one that does not declare roots/flat-skills/hooks, suppresses
  all three. The curator-supplied pin is NOT gated: it
  is authoritative regardless of the nested `mind.toml` (DSC-65). `as` (DSC-39)
  and `install` (DSC-58) are registry/consumer concerns and are likewise
  unaffected by this gate; they always apply.
- `DSC-61` A curator-supplied entry behaves as if the source had declared the
  same in its own `mind.toml`: when applied (the DSC-60 gate permits, i.e. the
  nested source ships no `mind.toml`), `roots` governs convention discovery
  (DSC-50) and the supplied hooks run under the same disclosure and safety prompt
  as a source's own hooks (HOOK-50..60), including the non-TTY skip and
  `--dangerously-skip-install-hook-check`. The curator-supplied pin always
  applies (DSC-65): it resolves and is recorded as the source's pin directive
  (DSC-41), so `sync` tracks it. A consumer's explicit `meld` pin flag still
  overrides a curator-supplied pin (DSC-41 precedence).
- `DSC-65` A pin directive on a `[discover].sources` entry (DSC-59) is
  authoritative: it sets the nested source's pin whether or not the source ships
  its own `mind.toml`, overriding the source's own `[source]` pin directive
  (DSC-41). It is exempt from the DSC-60 fallback gate, which governs only `roots`,
  `flat-skills`, and `hooks`; a nested entry that supplies only a pin (no gated
  roots/flat-skills/hooks) does not trigger the DSC-60 "ignored" warning. The precedence is: a consumer's direct
  top-level `meld` pin flag wins (DSC-41), then the entry's pin directive, then the
  source's own `[source]` pin directive, then the default branch. `dump` relies on
  this to reproduce each melded source at its recorded commit by emitting a
  per-entry `pin-ref` (DUMP-1, DUMP-4).
- `DSC-54` Melding a super-source (one whose `mind.toml` lists `[discover].sources`)
  registers the whole nested chain (DSC-38), but the post-meld auto-install flow
  (CLI-23) runs only over the super-source's OWN items (`<source>#*`) plus any
  nested source the curator marked `install = true` (DSC-58): the remaining nested
  discovered sources are registered and their items are left available, not
  installed. A super-source that ships its own items still offers them for install
  like any source; a purely curated registry installs nothing by default unless it
  flags an entry `install = true`.
- `DSC-55` `meld --recursive` (`-r`) extends the auto-install flow to EVERY nested
  discovered source, beyond the curator's `install = true` defaults: each source in
  the curated chain has its items offered for install via the same
  preview-and-prompt as the top-level source (honoring `--yes`). It applies both on
  a fresh meld and on a re-meld of an already-registered super-source: on a re-meld
  the chain is already registered, so its items are installed without
  re-registering. Without the flag only the top-level source's items and the
  `install = true` entries are offered (DSC-54, DSC-58). `--link-only` (register,
  install nothing) takes precedence: combined with `--recursive` it still installs
  nothing.
- `DSC-56` After a successful `meld` of a source that declares `[discover].sources`,
  `mind` prints a one-time advisory note pointing the user to `mind probe` to
  browse and search what the newly registered sources offer, so a curated registry
  is discoverable right after melding. The note prints after the install step.
- `DSC-57` `sync` re-walks each registered source's `[discover].sources` from its
  refreshed `mind.toml` and melds any newly-listed nested source not already
  registered, register-only (the DSC-54 default, never auto-installing nested
  items) and cycle-safe by the DSC-38 guards, so a curated registry picks up
  sources added upstream without a re-meld. A nested source removed from the list
  is left registered (removal stays an explicit `unmeld`): `sync` only adds.
- `DSC-66` Pin/ref values supplied in a `mind.toml` `[source]` or
  `[[discover.sources]]` pin directive (`follow-branch`, `pin-tag`, `pin-ref`)
  are validated at parse time before any git subprocess is invoked. A value that
  is empty, begins with `-`, contains ASCII whitespace, contains control
  characters, or contains `..` is rejected with `MindError::InvalidRef`. This
  prevents argument injection: a malicious or misconfigured `mind.toml` shipped
  by a melded super-source cannot inject git options (e.g.
  `--upload-pack=touch /tmp/pwned`) into a child `git` process. At the git call
  layer, a `--` end-of-options terminator is inserted before every positional
  ref/branch/tag/sha argument and before the repository URL in `git clone`
  invocations, so that git cannot interpret a value as an option even if
  validation were bypassed. The `prefer_ssh` rewrite applies to both `https://`
  and `http://` remotes so that a plain-HTTP clone URL is not left unrewritten
  when SSH is preferred.
- `DSC-40` When a source's `[source].min-mind-version` is greater than the
  running `mind` version, melding or scanning that source is an error
  (`IncompatibleVersion`) rather than proceeding against a format it predates.
  Versions compare by dotted numeric component (a missing component is 0, so
  `0.2` == `0.2.0`). The `min-mind-version` field value is validated at parse
  time (`MindToml::load`): it must be a non-empty string of one or more
  dot-separated components where every component is non-empty and consists
  solely of ASCII decimal digits (e.g. `"1"`, `"0.7"`, `"2.3.1"`). A value
  with an empty component, a non-digit character, or an empty string is
  rejected with a `MindToml` error naming the field and the bad value (e.g.
  `"0.3-beta"`, `"abc"`, `""`, `"1.x"` are all rejected). The running
  binary's own version string (from the build environment) may carry a
  pre-release segment (e.g. `0.2.0-rc1`); such a segment compares as 0 in
  the version comparison only. The gate (`IncompatibleVersion`) is enforced when
  the source is scanned: a binary older than the declared version refuses to
  scan it, so `meld`, `sync`, `recall`, and `probe` all fail rather than operate
  on a source the binary is too old for.
- `DSC-41` `[source]` may declare a pin: exactly one of `follow-branch = "<branch>"`,
  `pin-tag = "<tag>"`, or `pin-ref = "<commit>"`. It is read from the source's
  default-branch `mind.toml` and supplies the default pin when the consumer gives
  no `--follow-branch` / `--pin-tag` / `--pin-ref` flag at meld (CLI-17); a
  consumer flag overrides it. Declaring more than one is a `MindToml` error. (See
  CLI-18 for clone behavior and CLI-55 for how `sync` treats each pin kind.)

## Scan roots

By default convention discovery (DSC-10..12) scans the repo root. A monorepo, or
a repo whose agent tooling lives in a subtree, can point the scan at one or more
subdirectories instead.

- `DSC-50` `[source].roots` is an optional list of repo-root-relative directories.
  When set, convention discovery scans for `skills/`, `agents/`, `rules/` under
  *each* listed root rather than at the repo root. Unset means a single implicit
  root of the repo root (the DSC-10..13 behavior, unchanged). An explicitly empty
  list (`roots = []`) is distinct from unset: it scans zero roots and so
  discovers nothing.
- `DSC-51` `meld --root <dir>` (repeatable) overrides `[source].roots` entirely:
  convention discovery scans only the consumer-specified roots, letting a consumer
  narrow a broad source to exactly the subtree they want. The override is persisted
  on the source (STO-17) and applied by later scans and `sync`.
- `DSC-52` Scan roots affect convention discovery only. An authoritative `mind.toml`
  (one declaring `[[items]]` or `[discover]` item globs, DSC-3) keeps its
  repo-root-relative paths and ignores `roots`; if `--root` is passed for such a
  source, `meld` prints a note that it is ignored. A `--root` or `[source].roots`
  path that is not a directory in the clone is an error (`InvalidRoot`).
- `DSC-53` When scanning multiple roots, results are unioned. Two roots that yield
  the same kind and bare name within one source is an error (`DuplicateItem`),
  since an item's identity is `(source, kind, bare_name)` and the collision could
  not be installed unambiguously. The same uniqueness check applies to explicit
  `[[items]]` declarations: two entries with the same kind and name in a
  `mind.toml` are a `DuplicateItem` error.

## Flat skill layout

By default a skill is a directory under a `skills/` container (DSC-10). A source
whose skill directories sit directly at a scan root, with no `skills/` container,
can opt into flat skill discovery instead of having to spell out a
`[discover].skills` glob.

- `DSC-74` `[source].flat-skills` is an optional boolean (default false). When
  true, convention discovery finds a skill as a bare-name directory containing a
  `SKILL.md` directly under a scan root (`<root>/<name>/SKILL.md`), taking the
  directory name as the bare skill name, rather than requiring the `skills/`
  container (DSC-10). The scan is shallow: only the immediate child directories of
  each root are checked for a direct `SKILL.md`. It composes with `roots` (DSC-50):
  with `roots` unset the single implicit root is the repo root, so flat discovery
  scans `<repo>/*/SKILL.md`; with `roots` set it scans each listed root. Flat
  discovery applies to the skill kind only: the `SKILL.md` anchor disambiguates a
  skill directory from an arbitrary one, whereas agent and rule items are bare
  `.md` files and tool directories carry no required anchor, so a bare directory at
  a root cannot be classified for those kinds. Agent (`agents/`), rule (`rules/`),
  and tool (`tools/`) discovery under each root is unchanged. False or unset is the
  DSC-10 container behavior. Flat discovery changes only the discovered on-disk
  path and the derived bare name; an item's stable identity, store path, and link
  target are unchanged (a flat skill `<root>/foo/` installs and links identically
  to a containered `skills/foo/`), so install, link, uninstall, drift, and the
  `recall`/`probe` kind categorization (the item's `kind` is assigned at scan, not
  inferred from its source path) need no layout-specific handling.
- `DSC-75` `meld --flat-skills` force-enables flat skill discovery for the source:
  it turns the flat layout on even for a source that did not declare
  `[source].flat-skills`. The flag is one-directional (there is no
  `--no-flat-skills`): the effective setting is the consumer override OR the
  source's own `[source].flat-skills` (STO-44, DSC-74), so a consumer cannot
  disable a source's declared flat layout (which would make its skills
  undiscoverable). It is persisted on the source (STO-44) and applied by later
  scans and `sync`. Like `--root` (DSC-51), it is set at the initial meld; a
  re-meld of an already-registered source does not change the persisted value
  (unmeld and meld again to change it). Like `roots`, it affects convention
  discovery only.
- `DSC-76` Flat skill discovery affects convention discovery only. An authoritative
  `mind.toml` (one declaring `[[items]]` or `[discover]` item globs, DSC-3) keeps
  its explicit paths and ignores `flat-skills`; if `--flat-skills` is passed for
  such a source, `meld` prints a note that it is ignored (mirroring the DSC-52
  treatment of `--root`).
- `DSC-77` A `[discover].sources` entry may set `flat-skills = true` to declare
  that a nested source uses the flat skill layout, alongside the other
  curator-supplied configuration a source would normally declare in its own
  `mind.toml` (`roots`, hooks, a pin; DSC-59). Like `roots` and hooks, it is
  applied only when the nested source ships no `mind.toml` of its own; when the
  nested source has a `mind.toml`, that file is authoritative and the
  curator-supplied `flat-skills` is ignored with a warning (the DSC-60 gate). This
  lets a curator adopt a flat-layout source that has not onboarded itself without
  forking it.

- `DSC-78` A `[discover].sources` entry uses the key `namespace` (not `as`) to
  declare the alias to impose on the nested source; `as` is retained as a
  backward-compatible alias for `namespace` but `namespace` is the canonical form.
  `dump` always emits `namespace =` (never `as =`) so round-tripped manifests use
  the canonical key. `effective_alias()` returns the `namespace` value when present,
  falling back to the legacy `as` value, so both keys work at meld and sync time.

## Authentication failure handling for nested sources

A curator may list sources that require authentication (private GitHub repos,
self-hosted forges). By default a git auth failure during meld or sync is a hard
error indistinguishable from any other network error. The `on-auth-failure` entry
field lets the curator opt in to named handling.

- `DSC-67` *(removed: `private = true` flag was dropped before implementation in
  favor of the inline-table form in DSC-68)*

- `DSC-68` A `[discover].sources` entry may set `on-auth-failure` as an inline
  table with a required `action` key and an optional `message` key.
  `action` must be `"error"` or `"skip"`. `message`, when present, is a plain
  string shown to the user alongside the standard auth-failure line (DSC-69).
  Without `on-auth-failure`, an auth failure is a generic git error (hard
  error, non-zero exit). Authentication
  failure is detected by matching the following credential-denial patterns in
  the git subprocess stderr (case-insensitive): `authentication failed`,
  `permission denied (publickey)`, `could not read username`,
  `could not read password`,
  `the requested url returned error: 401`,
  `the requested url returned error: 403`, `invalid username or password`,
  `invalid credentials`, `http basic: access denied`,
  `fatal: unable to authenticate`. Detection depends on English-language git
  stderr; under a non-English locale (`LANG`/`LC_ALL` not `en`), patterns may
  not match and the entry degrades to the generic hard-error path. Patterns
  that conflate access denial with a missing repository (e.g. `repository not
  found`) are intentionally excluded. The same handling applies during `sync`,
  which re-walks `[discover].sources` through the same path (DSC-57); a
  skipped nested source is warned and left unregistered, and
  `action = "error"` fails the sync. Setting `action = "error"` vs. omitting
  `on-auth-failure` entirely: both exit non-zero on auth failure, but
  `"error"` uses the standardized DSC-69 message format and supports the
  optional `message` field; omitting `on-auth-failure` produces a raw generic
  git error with no custom message. With it, the action governs: `"error"`
  still exits non-zero; `"skip"` emits a warning and continues. The source is
  not registered and any transitive chain reachable only through it is also
  skipped.

  ```toml
  [[discover.sources]]
  source = "owner/private-repo"
  on-auth-failure = { action = "skip", message = "Configure credentials: https://example.com/auth" }
  ```

- `DSC-69` When `on-auth-failure` is set and auth fails, `mind` always prints
  `"unable to meld source <source> due to authentication failure"` (where
  `<source>` is the parsed short name, the `name` field derived from
  `parse_spec`, not the full URL or spec string the curator wrote), with
  `" (skipping)"` appended when `action` is `"skip"`. If `message` is set, it is
  printed on the line immediately following. Both the source name and the
  curator `message` have ANSI escape sequences and non-printable control
  characters stripped before display, preventing terminal injection from
  curator-controlled content. Under `"error"`, the message (if any)
  is printed before the process exits non-zero. Under `--json`, skipped entries
  appear as objects in a `"skipped"` array on the outer mutation result, each
  object carrying `"source"` and `"reason": "auth_failure"`. No separate JSON
  object is emitted per skipped source; the outer result is one JSON object
  total, consistent with CLI-153.

- `DSC-70` `on-auth-failure` governs only the direct clone failure of the entry
  itself; auth failures from transitive descendants (sources nested within the
  entry's own `mind.toml`) propagate as hard errors regardless of the entry's
  policy. The implementation detects descendant failures by checking whether the
  entry's source is already present in the registry at the point the auth failure
  arrives: a registered entry cloned successfully, so the failure originates from
  a deeper level. The same scoping applies during `sync` re-walk.

- `DSC-79` — A `[discover].sources` entry whose clone fails for a non-auth reason (network
  error, not-found, etc.) is skipped with a stderr warning rather than hard-failing the meld;
  the primary source and every successfully-cloned nested source remain registered. The skip is
  surfaced in the `SkippedEntry` list (`--json` `skipped[]`) with `reason = "clone_failure"`.
  Scoping follows DSC-70: a failure arriving after the entry is already in the registry
  originates from a descendant and propagates unchanged. The same skip applies during `sync`
  re-walk.
- `DSC-80` — When the primary source is exclusively a curator (a catalog scan of its own
  directory yields zero items) and every nested `[discover].sources` entry fails to register,
  the meld hard-fails: registering a source with zero discoverable items is not useful. A
  primary source with at least one of its own items, or with at least one nested source that
  registered, succeeds (exit 0). The primary/top-level source's own clone failure is a hard
  error regardless (unchanged).

## Glob confinement and name safety

- `DSC-81` A `[discover]` glob pattern must be a relative path: a pattern that
  is absolute or contains a `..` path component is rejected at `mind.toml` parse
  time with a `MindToml` error. Every filesystem path produced by expanding a
  `[discover]` glob is canonicalized and checked to lie within the clone root; a
  match outside the root is an error (mirrors the DSC-73 treatment of explicit
  `[[items]]` paths). Glob-discovered item names are validated with the same
  safe-component rule as `[[items]]` names (DSC-71): an empty, `.`, `..`, or
  separator-bearing name is rejected with a `MindToml` error.

- `DSC-83` For skill globs in `[discover]`, the bare skill name is the parent
  directory name (not the `SKILL.md` file stem). The safe-component check
  (DSC-81) is applied to the parent directory name so that a skill directory
  whose name contains a path separator or NUL is rejected at discovery time,
  consistent with how `[[items]]` names are validated (DSC-71).

## `[source].namespace`

- `DSC-82` The `[source]` table accepts both `namespace` and `prefix` for the
  namespace prefix field. Both keys name the same field; `namespace` is the
  canonical key (written by `init-source` per INIT-11, matching the nested-entry
  key per DSC-78 and the `--namespace` flag per CLI-159) and `prefix` stays
  accepted indefinitely as a deprecated parse alias. When a `mind.toml` carries
  `prefix =` and is rewritten by `init-source`, the `prefix =` line is replaced
  with `namespace =` so only one key is present after the update.
