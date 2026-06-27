# Super-source example

A source repo that curates other sources. A `[discover].sources` list names the
repos to meld, so melding this one registers the whole chain.

## What it shows

A curated registry. `[discover].sources` lists other sources this repo melds.
Melding this repo registers each listed source as well, so one `meld` brings up
the whole chain.

A bare `[discover].sources` list (no `[[items]]` and no `[discover]` kind globs)
is NOT authoritative, so convention scanning stays on. This repo therefore ships
its own items too: the `onboard` skill under `skills/` is discovered alongside
the curated sources. That is the "regular plus super-source" combination.

Per-entry knobs on a nested source:

* `as = "rev"` imposes a namespace prefix on that nested source, exactly like
  `meld --as`. Its items install as `rev-<name>`.
* `install = true` offers that nested source's items for install when the
  super-source is melded; the curator recommends installing it. Default is
  `install = false`, which only registers the source and leaves its items
  available but not offered.

By default, melding a super-source installs only its own items plus the
`install = true` entries. Add `meld --recursive` (or `-r`) to offer every nested
source for install.

The two forms are equivalent:

```toml
[discover]
sources = [
    { source = "../tooling", install = true },
]
```

```toml
[[discover.sources]]
source = "../tooling"
install = true
```

## Layout

```
skills/onboard/SKILL.md   this repo's own convention item (description in frontmatter)
mind.toml                 [source] metadata + [discover].sources curating the chain
```

`mind.toml` lists four nested sources: a plain entry left available, an `as =`
entry namespaced to `rev`, an `install = true` entry offered on meld, and an
adopt-an-un-onboarded entry that supplies `follow-branch`, `roots`, and a build
hook for a source that ships no `mind.toml` of its own (DSC-59/60/61). The nested
specs are local paths (`../explicit` and friends) to the sibling example repos in
this tree, so the file is safe to read and copy; a real super-source lists remote
specs in the same positions.

## Try it

This directory mainly demonstrates the `[discover].sources` shape; the `review`
verb validates it with no network:

```
mind review examples/super-source
```

The nested specs are local paths (`../explicit`, `../namespacing`, `../tooling`,
`../starter`) that resolve against the sibling example repos in this `examples/`
tree, so a `meld` of this directory from a checkout registers those siblings as
the chain. To curate your own chain, replace each `source` with a remote spec you
control (`owner/repo`, `git@host:owner/repo`, or a URL) and meld:

```
mind meld <path-or-repo>      # registers this repo and the whole chain
mind probe                    # browse what the chain offers
```

## Verified

`tests/cli.rs::example_super_source_validates` runs `review` on this directory
and asserts it validates, so the example stays correct as the code changes.
