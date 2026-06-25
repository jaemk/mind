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
  top-level scalar keys and surrounding quotes. Block scalars and nested
  structures are not interpreted. No frontmatter yields no description.

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
  { source = "github:foo/bar", as = "fb" },   # impose a namespace on the nested source
]
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
- `DSC-54` Melding a super-source (one whose `mind.toml` lists `[discover].sources`)
  registers the whole nested chain (DSC-38), but the post-meld auto-install flow
  (CLI-23) runs only over the super-source's OWN items (`<source>#*`): the nested
  discovered sources are registered and their items are left available, not
  installed. A super-source that ships its own items still offers them for install
  like any source; a purely curated registry installs nothing by default.
- `DSC-55` `meld --install-super-sources` extends the auto-install flow to the
  nested discovered sources: each source in the curated chain has its items
  offered for install via the same preview-and-prompt as the top-level source
  (honoring `--yes`). It applies both on a fresh meld and on a re-meld of an
  already-registered super-source: on a re-meld the chain is already registered,
  so its items are installed without re-registering. Without the flag only the
  top-level source's items are offered (DSC-54). `--link-only` (register, install
  nothing) takes precedence: combined with `--install-super-sources` it still
  installs nothing.
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
