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
min-mind-version = "0.2"     # reserved; parsed, not yet enforced

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
  already registered; cycles are guarded by URL so recursion terminates. This
  lets a `mind.toml` act as a curated registry / super-source. Each curated
  source is registered independently and tracks its own upstream commit.
- `DSC-39` A `[discover].sources` entry may set `as = "<prefix>"` to impose a
  namespace on that nested source (equivalent to `meld --as`).
- `DSC-40` (planned) When a source's `[source].min-mind-version` is greater than
  the running `mind` version, melding or scanning that source is an error
  (`IncompatibleVersion`) rather than proceeding against a format it predates.
  Currently the field is parsed but not enforced. Not yet implemented.
