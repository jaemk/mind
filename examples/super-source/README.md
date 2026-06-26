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
    { source = "acme/recommended-rules", install = true },
]
```

```toml
[[discover.sources]]
source = "acme/recommended-rules"
install = true
```

## Layout

```
skills/onboard/SKILL.md   this repo's own convention item (description in frontmatter)
mind.toml                 [source] metadata + [discover].sources curating the chain
```

`mind.toml` lists three nested sources: a plain entry left available, an `as =`
entry namespaced to `rev`, and an `install = true` entry offered on meld.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/super-source /tmp/super-demo
cd /tmp/super-demo && git init -q && git add -A && git commit -qm init
```

`mind meld /tmp/super-demo` would clone every repo named in `[discover].sources`,
so the placeholders here (`acme/agent-lib` and friends) only work if you swap in
real repos you control. With those in place:

```
mind meld /tmp/super-demo     # registers this repo and the whole chain
mind probe                    # browse what the chain offers
```

## Verified

`tests/cli.rs::example_super_source_validates` runs `review` on this directory
and asserts it validates, so the example stays correct as the code changes.
