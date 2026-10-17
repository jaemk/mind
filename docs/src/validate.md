# Validate before publishing

Two commands help a maintainer: `mind review` checks a source the way a
consumer's meld would see it, and `mind init-source` scaffolds a `mind.toml` and
reports how the items reference each other.

```
mind review                  # check the source in the current directory
mind init-source             # scaffold mind.toml and report references
```

## review

```
mind review                      # the current directory
mind review ./path/to/repo       # a local path
mind review owner/repo           # a melded source, or a repo to shallow-clone
mind review . --namespace jk     # check it as it would install under a namespace
mind review . --fix              # rewrite what it can fix, in place
```

`review` changes nothing except under `--fix`. A path to an existing directory
is read as that directory, unless it first matches a melded source's name.

Findings are hard (non-zero exit) or advisory (exit 0):

**Hard**

- a malformed `mind.toml`, or an unknown item kind
- a `min-mind-version` newer than the running `mind`
- an unresolved `{{ns:}}`, `{{self}}`, `{{tools:}}`, or `{{path:}}` reference
- an `expand:` entry naming a file the item does not ship
- an invalid install profile, a profile cycle, or a profile member naming no
  item

**Advisory**

- a missing description
- a hardcoded install path (`hardcoded-path`); see
  [Source layout](source-layout.md#hardcoded-paths)
- a sibling tool named in prose without a token
- a `{{ns:}}` token in a code block or path, where it does not belong
- an item that names a sibling in plain text when a namespace is in effect
- the same helper script copied into several items (`duplicate-tooling`)
- a deprecated `[source].install` field
- each declared source or item hook, so a consumer sees the source runs code
- workflow defects the harness would reject (see [Workflows](workflows.md))

`--fix` works on a local working tree only. It replaces recognized hardcoded
paths with tokens, unwraps misplaced `{{ns:}}` tokens, and wraps bare sibling
names in `{{ns:}}`. It leaves structural findings such as `duplicate-tooling`
to you, and leaves workflow `.js` files alone.

`review --json` returns the findings as one document; see
[Scripts and CI](scripting.md#json-output). Run it in your source's CI to catch
breakage before consumers do:

```
mind review . --json
```

`review --policy <path>` validates a managed policy file instead; see
[Managed policy](policy.md#validation).

## init-source

```
mind init-source                 # report and scaffold
mind init-source path/to/repo
mind init-source --template      # also rewrite bare sibling names to {{ns:}} tokens
mind init-source --marketplace   # also generate .claude-plugin/marketplace.json
mind init-source --flat-skills   # skills sit at the repo root
mind init-source -N acme         # set the namespace in the scaffold
```

Each run:

1. Discovers the items exactly as a meld would, so what it reports is what a
   consumer gets.
2. Writes a `mind.toml` if there is none, with a `[source]` table and a
   description placeholder. An existing `mind.toml` is never overwritten.
3. Reports each item's references to its siblings: those already written as
   `{{ns:name}}` tokens, and bare mentions in prose. Bare mentions are reported
   as `unguarded-reference` advisories only when a namespace is in effect,
   since without one they resolve as written.
4. Reports `duplicate-tooling` for a helper file copied byte-for-byte into two
   or more items.

`--template` rewrites each bare whole-word sibling name in item text to its
`{{ns:name}}` token. It skips text already in a token, code blocks, inline
code, paths, and frontmatter fields, and leaves workflow `.js` files alone.
Sibling names are often ordinary words, so review the result with `git diff`
before committing.

`init-source` makes no network calls and touches no store or agent home.
Without `--template` it only creates an absent `mind.toml`; with it, it edits
item files in the target repo.
