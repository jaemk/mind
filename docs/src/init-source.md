# init-source

`mind init-source [path]` helps a source author prepare a repo for melding. It
scaffolds a `mind.toml`, reports the intra-source reference graph it detects,
and optionally rewrites bare sibling references into `{{ns:}}` tokens. It is
the setup counterpart to `review` (which validates without changing anything).

## What it does

Run `mind init-source` in the repo (or pass a path):

```
mind init-source                 # report + scaffold
mind init-source --template      # also rewrite bare sibling refs to {{ns:}}
mind init-source path/to/repo
```

On each run it:

1. Discovers items exactly as melding would: convention scanning
   (`skills/<n>/SKILL.md`, `agents/<n>.md`, `rules/<n>.md`, `tools/<n>/`), then
   an authoritative `mind.toml` if one is present. What it reports is what
   melding would install.
2. Scaffolds a `mind.toml` when none exists (INIT-3). The scaffold contains a
   `[source]` table with a `description` placeholder and a commented-out
   `prefix`. An existing `mind.toml` is never overwritten.
3. Reports the intra-source dependency graph (INIT-4): for each item, the
   siblings it references via existing `{{ns:name}}` tokens, and the siblings it
   mentions in bare prose. Bare-prose mentions are emitted as
   `unguarded-reference` advisories in the same `advisory [kind]: message`
   format that `review` uses, and only when a prefix is in effect (see
   [below](#bare-reference-advisories)).
4. Reports `duplicate-tooling` advisories for any non-markdown helper file that
   is byte-identical across two or more items (INIT-7), in the same finding
   format as `review`. These are advisory only; `--template` does not fix them.
   Extracting a shared tool into `tools/<name>/` is a structural change made by
   hand.

## `--template`: rewriting bare references

With `--template`, `init-source` rewrites each bare whole-word sibling mention
in item text to its `{{ns:name}}` token (INIT-5), writes the changed files, and
reports each rewrite.

What is rewritten:

- Bare whole-word occurrences of a sibling item's name in prose.

What is left alone:

- Text already inside a `{{ns:}}` token.
- Names inside a fenced code block, an inline code span, a path, or a
  frontmatter structured field. A keyword or path component is never wrapped.

The rewrite is heuristic: a sibling's name can be an ordinary English word, so
the result should be reviewed (e.g. with `git diff`) before committing.

See [Namespacing](namespacing.md) for how `{{ns:name}}` tokens work and when
they are needed.

## Bare-reference advisories

`init-source` emits `unguarded-reference` advisories only when an effective
prefix is in force for the repo (`[source].prefix` in `mind.toml`) (INIT-9).
Without a prefix, bare sibling references resolve as written at runtime, so
flagging them would be noise. This matches `meld` and `review`, which also
suppress this advisory on unprefixed sources.

`{{ns:}}`-token edges in the dependency graph and the `--template` rewrite are
unaffected by this gate; only the advisory is gated.

## Safety contract

`init-source` makes no network calls and does not read or write the store or
any agent home (INIT-6). Without `--template` it is read-only except for
creating an absent `mind.toml`. With `--template` it edits item files in the
target repo only.

## Broader workflow

`init-source` sets up the repo; `review` validates it before publishing. The
full authoring workflow is in [Authoring a source](authoring.md).
