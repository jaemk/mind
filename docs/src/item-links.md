# Install one item from a link

Paste the URL of a skill straight from GitHub or GitLab and `learn` installs
just that skill, without melding the rest of the repo:

```
mind learn https://github.com/owner/repo/tree/main/skills/foo
mind learn https://github.com/owner/repo/blob/main/skills/foo/SKILL.md
```

A link to any other `.md` file installs that one file as an agent, rule, or
command:

```
mind learn https://github.com/owner/repo/blob/main/agents/reviewer.md
mind learn https://github.com/owner/repo/blob/main/rules/style.md
```

## How the kind is decided

The first of these that applies wins:

1. `--kind agent`, `--kind rule`, or `--kind command` on the command line.
2. The file's directory: `agents/`, `rules/`, or `commands/`.
3. A `kind:` key in the file's own frontmatter.

A link into a conventional layout needs no annotation. A file elsewhere needs
`--kind`:

```
mind learn https://github.com/owner/repo/blob/main/vendor/reviewer.md --kind agent
```

Skills and tools are directories, so `--kind` never names them; a directory link
is always a skill. A workflow (`workflows/<name>.js`) cannot be installed from a
link, since a file link takes `.md` files only. Meld the repo and
`learn workflow:<name>` instead.

`--kind` is fixed when the link is first registered. Running it again against
an already-registered link changes nothing, and `mind` says so.

## What gets registered

The link registers as its own single-item source with the identity
`host/owner/repo#<path>`, for example `github.com/owner/repo#skills/foo`. It
clones, syncs, and upgrades like any source, but offers exactly the linked
item. Several links into the same repo, and a plain meld of that repo, coexist
as separate sources.

The URL's ref sets the version:

- a branch name (`tree/main/...`) follows that branch; `upgrade` picks up new
  commits.
- a 40-character commit pins it.
- `--pin` freezes a branch link at the branch's current commit:

```
mind learn https://github.com/owner/repo/tree/main/skills/foo --pin
```

`mind meld <url>` accepts the same URLs and runs the normal meld flow
(`--register-only`, `--namespace`, `--kind`, `--pin <value>`). A local repo is
reachable as `file:///path/to/repo/tree/<branch>/<path>`.

Because you name the exact path, a link reaches items the repo's own inventory
does not list, such as a skill left out of an authoritative `mind.toml` or a
`.claude-plugin/marketplace.json`.

## Removing it

`mind forget <item>` uninstalls the item and leaves the link's source
registered, with a hint to drop it:

```
mind unmeld 'github.com/owner/repo#skills/foo'
```

## When the item needs its siblings

A link offers one item, so an item that references others in its repo cannot
get them this way. That covers a `requires:` frontmatter entry and a `{{ns:}}`,
`{{tools:}}`, or `{{path:}}` token (see
[Dependencies between items](dependencies.md)).

- A `requires:` entry is metadata. The item installs, `mind` warns which
  entries it dropped, and `mind recall <item>` and `mind introspect` keep
  showing the gap.
- A token would be written into the item's text, so it is an error and nothing
  installs.

Both print a command that swaps the link for a meld of the whole repo and
installs that one item with what it needs:

```
mind unmeld 'github.com/owner/repo#skills/foo' --yes && \
  mind meld https://github.com/owner/repo --learn 'skill:foo' --yes
```

When the repo's own inventory (an authoritative `mind.toml` or plugin manifest)
does not list the item, the command adds `--add-root`, derived from the link's
path:

```
mind unmeld 'github.com/owner/repo#skills/foo' --yes && \
  mind meld https://github.com/owner/repo --add-root '.' --learn 'skill:foo' --yes
```

A file link outside its kind's directory (`vendor/reviewer.md`) has no such
remedy, since a whole-repo meld would never find it; the error says to drop the
reference or move the file under `agents/` upstream. The printed command does
not carry over a `--namespace`, `--pin`, or extra `--add-root` the link was
melded with; add those yourself.

## Links in a curated list

A curator can list an item link in `[discover].sources`, with `kind` when the
file needs it:

```toml
[[discover.sources]]
source = "https://github.com/owner/repo/blob/main/vendor/reviewer.md"
kind = "agent"
install = true
```

`mind dump` writes link sources back out as URLs, so a setup that includes them
reproduces. Spec:
[spec/item-link.md](https://github.com/jaemk/mind/blob/main/spec/item-link.md).
