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

## mind.toml

```toml
[source]
description = "..."          # optional; shown by recall --sources
prefix = "jk"                # optional; namespace (see namespacing.md)
min-mind-version = "0.2"     # minimum mind version; enforced at scan/meld (DSC-40)
roots = ["packages/tools"]   # optional; convention scan roots (DSC-50)
follow-branch = "main"       # optional; pin directive, one of
                             #   follow-branch / pin-tag / pin-ref (DSC-41)

[[items]]                     # explicit inventory (authoritative)
kind = "skill"               # skill | agent | rule
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
follow-branch = "main"           # pin directive for the nested source (DSC-41)
roots = ["packages/agents"]      # scan roots for the nested source (DSC-50)
[[discover.sources.hooks]]       # build hooks for the nested source (HOOK-50)
run = "make build"
```

- `DSC-30` Unknown top-level or table fields are rejected (the file is strict).
- `DSC-31` A `[[items]]` entry with an unknown `kind` is an error (`MindToml`).
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
- `DSC-59` (planned) A `[discover].sources` entry may carry configuration for the
  nested source that the source itself would normally declare in its own
  `mind.toml`: `follow-branch = "<branch>"` (a pin directive, DSC-41), `roots =
  [...]` (convention scan roots, DSC-50), and one or more hooks as a
  `[[discover.sources.hooks]]` array-of-tables (the `[[hooks]]` shape, HOOK-50).
  This lets a curator add support for a source that has not onboarded itself
  (no `mind.toml`), including one with custom build requirements or a monorepo
  layout, without forking it.
- `DSC-60` (planned) The curator-supplied `follow-branch`, `roots`, and `hooks`
  (DSC-59) apply only when the nested source ships no `mind.toml` of its own. When
  the nested source has a `mind.toml`, that file is authoritative for its pin,
  roots, and hooks and the curator-supplied values are ignored (a warning is
  emitted, since the source has onboarded). The gate is whole-file: a nested
  `mind.toml`, even one that does not declare a pin/roots/hooks, suppresses all
  three. `as` (DSC-39) and `install` (DSC-58) are registry/consumer concerns and
  are unaffected by this gate; they always apply.
- `DSC-61` (planned) When applied (DSC-60), a curator-supplied entry behaves as if
  the source had declared the same in its own `mind.toml`: `follow-branch`
  resolves and is recorded as the source's pin directive (DSC-41), so `sync`
  tracks that branch; `roots` governs convention discovery (DSC-50); and the
  supplied hooks run under the same disclosure and safety prompt as a source's own
  hooks (HOOK-50..60), including the non-TTY skip and
  `--dangerously-skip-install-hook-check`. A consumer's explicit `meld` pin flag
  still overrides a supplied `follow-branch` (DSC-41 precedence).
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
- `DSC-40` When a source's `[source].min-mind-version` is greater than the
  running `mind` version, melding or scanning that source is an error
  (`IncompatibleVersion`) rather than proceeding against a format it predates.
  Versions compare by dotted numeric component (a missing component is 0, so
  `0.2` == `0.2.0`); a non-numeric component counts as 0.
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
  not be installed unambiguously.
