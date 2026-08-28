# Ignored files

Status: done. Which files under an item's path are excluded from the store
copy and from the content hash, so an item can point at a directory that holds
more than the item itself.

## Overview

An item's path names a directory (a skill or tool) whose entire tree is copied
into the store at install and walked to compute the content hash that drives
drift and `upgrade` (LIFE-15). Today that tree is taken whole: `install.rs`'s
`copy_recursive` and `hash.rs`'s `collect_files_at` both walk every entry with
no exclusions.

That is fine while an item's directory holds only the item. It breaks as soon as
the item's path IS the repo root, which a source declares to ship a top-level
`SKILL.md`:

```toml
[[items]]
kind = "skill"
name = "root-skill"
path = "."
```

The store copy then contains `.git/`, and the hash covers it, so the item reads
as drifted after any local commit, fetch, or even a `git gc` in the source
clone. `upgrade` offers a change that is not a change to the skill.

The fix is an ignore mechanism with one rule shared by both walks: what is not
installed is not hashed. Ignores are source truth, declared in `mind.toml`, so
the hash of an item is a property of the source and not of the machine that
computed it.

## What is ignored

- `IGN-1` An item's ignore set is the built-in set (IGN-2) plus exactly ONE
  declared list: the item's own `[[items]].ignore` if it declared one, else the
  source-level `[source].ignore`. There is no union of the two declared lists.
  An item-level list replaces the source-level list for that item rather than
  adding to it, matching how `[[items]]` overrides `[source]` elsewhere; the
  built-in set applies either way. `ignore = []` on an item is the explicit
  opt-out of a source-wide list: it declares an (empty) item-level list, so
  nothing from `[source].ignore` carries over. Patterns are matched against
  paths relative to the ITEM's path, not the repo root.
- `IGN-2` The built-in set is the version-control metadata directories `.git`,
  `.hg`, `.svn`, and `.bzr`, excluded from every item's copy and hash whether or
  not the source declares anything, whether each is a directory or (in a
  submodule, a `git worktree` checkout, or a `git init --separate-git-dir`
  layout) a plain file. `.git` is a regular file, not a directory, in exactly
  those three trees, and a dangling `.git` pointer file in the store is the
  same dead weight the directory would be. A source control directory or file
  inside an installed skill is at best dead weight in the store and at worst a
  second repository inside the user's agent home. This is the whole of the
  built-in set: build and dependency output (`target/`, `node_modules/`,
  `__pycache__/`) is NOT implied, since mind cannot tell a build directory from
  a directory a skill deliberately ships, and guessing would silently drop a
  file the author meant to install.
- `IGN-3` An entry is a glob matched against the item-relative path with `/` as
  the separator, so `*` does not cross a `/` and `**` does. This is NOT the
  matching the item selectors use (CLI-31): those match a flat item name, where
  a separator never arises, and so they take the `glob` crate's default, in
  which a bare `*` spans `/`. An ignore pattern names a path, so it is matched
  with `require_literal_separator`. A trailing `/` marks a
  directory-only match (`scratch/` ignores the directory and everything under
  it, and never a file named `scratch`); without it an entry matches either. A
  directory that matches is not descended into, so its subtree costs nothing to
  skip. Practical consequences of this rule:
  - (a) A pattern with no `/` anchors at the ITEM ROOT, so `target` matches
    only the top-level `target` and never `sub/target`. Write `**/target/` to
    exclude it at any depth.
  - (b) A trailing `/` is a directory-only match: `scratch/` never matches a
    file named `scratch`, while `scratch` matches either.
  - (c) `foo/` excludes the directory itself, while `foo/**` excludes only its
    contents, so `foo/**` leaves an empty `foo/` in the store copy (the hash is
    unaffected: directories do not contribute to it). `foo/` is almost always
    what is wanted.
- `IGN-4` An ignore entry is a safe relative path pattern: not absolute, not
  `~`-rooted, no `..` component, no NUL (the DSC-71..73 rule applied to a
  pattern). A violation is a hard `mind.toml` error at scan, named like the
  other malformed-declaration errors, rather than a silently inert entry. A
  malformed `ignore` arriving on a `sync` therefore fails every command for
  every source, not just the offending one: `catalog::scan` degrades
  gracefully only for `LinkedSourceGone`. This is the deliberate inherited
  default already shared with `DuplicateItem` and `InvalidRoot`.
- `IGN-5` An ignore entry may not exclude the item's own anchor file: a skill's
  `SKILL.md`, a tool's `TOOL.md`, or the single file that IS an agent or rule
  item. An entry that would is a hard error at scan. Without this an item could
  be declared and then install as an empty directory, which discovery would
  still offer and the harness would find unusable. A tool's `TOOL.md` is
  optional (TOOL-2), so this rule fires only when the anchor file EXISTS. For
  an agent or rule item the rule cannot fire at all: the item IS its file, so
  there is no path inside it for a pattern to match, and an ignore can never
  target it.

## Where it applies

- `IGN-10` The ignore set is applied identically by the install copy and by the
  content hash. This is the point of the feature, not an implementation detail:
  hashing a file that is not installed makes `upgrade` offer changes the user
  cannot see in the installed item, and installing a file that is not hashed
  makes a change to it invisible to drift detection. Both walks are already
  depth-capped (LIFE-52) and stay so. Both walks classify an entry with
  `symlink_metadata`, so a symlink is never a directory to either: a
  directory-only ignore rule matches a symlink in neither walk. An ignored
  entry is skipped rather than rejected, so LIFE-42's refusal of symlinks
  applies only to what the item actually contains, not to what `ignore`
  removed before that check runs.
- `IGN-11` A file excluded by the ignore set is invisible to every other pass
  that reads an item's tree: `{{ns:}}` / `{{tools:}}` / `{{path:}}` token
  expansion and its NS-57 `expand:` list, the reference scans (NS-20, LNK-18),
  `review`'s findings (CLI-130..139), Checks 13/14 (CLI-190/191), and the
  `duplicate-tooling` advisory (CLI-144). A file mind will not install cannot
  be a source of references mind resolves, and reporting a finding against a
  file the user will never receive is noise. The inverse is a stated
  limitation, not a checked error: an `ignore` that removes a file the item's
  own declarations depend on (a tool's resolved `bin`, a `{{self}}`/`{{path:}}`
  target) installs cleanly and breaks at RUNTIME. This differs from the
  `expand:` contradiction (IGN-12) and the missing anchor file (IGN-5), both of
  which are hard errors at scan.
- `IGN-12` `expand:` (NS-57) and `ignore` are contradictory when they name the
  same file: `expand:` asks for token expansion in a file that `ignore` removes
  from the item. That is an authoring mistake, so it is a hard error at scan
  naming both entries, not a silent precedence rule.
- `IGN-13` The ignore set does not affect DISCOVERY: convention scanning still
  finds items under a scan root (DSC-10..13) regardless of any `ignore`.
  `ignore` narrows what an item CONTAINS, never which items a source offers;
  `[discover]` globs and `roots` are the mechanisms for the latter. An item
  cannot exclude ITSELF either, and not by a rule that forbids it: patterns are
  matched against paths INSIDE the item, so the item's own path is never a
  candidate, and the empty relative path never matches (which is what IGN-5
  protects the anchor file within).

## Reproduction and migration

- `IGN-20` `dump` (DUMP-1) emits nothing for `ignore`, and does not need to. An
  ignore list is SOURCE truth: it lives in the source repo's own `mind.toml`, so
  re-melding the source at the pinned commit reads the same lists and installs
  the same file set, computing the same hashes. This is the difference between
  `ignore` and the consumer-side directives dump does emit (`roots`,
  `add-roots`, `flat-skills`, DUMP-4/10/11): those live in the CONSUMER's
  registry, set by a `meld` flag, and would be lost on reproduction if dump did
  not record them. There is deliberately no consumer-side ignore flag, since a
  hash that depended on a local flag would differ between two machines melding
  the same pinned source.
- `IGN-21` Adding the built-in set (IGN-2) changes the hash of any already
  installed item whose tree contains one of those names, as a directory or (in
  a submodule or worktree checkout) as a file, so such an item reports as out
  of date once and `upgrade` re-installs it without the VCS directory. That is
  a real content change (the store copy loses files) and is
  reported as an ordinary upgrade, not suppressed. In practice this reaches only
  items whose path is a repo root or a submodule, which is the case this feature
  exists to fix.
