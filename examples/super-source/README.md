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
the curated sources. That is the "regular plus super-source" combination. This
repo also sets `[source].prefix = "team"`, so its own `onboard` installs as
`team-onboard`.

### Per-entry install control (DSC-58/62/64)

Each `[discover].sources` entry chooses, independently, what melding the
super-source does with that nested source's items:

* omitted (`install` unset) registers the source and leaves its items available
  but not offered for install.
* `install = true` offers all of that source's items for install (DSC-58).
* `install-items = ["kind:name", ...]` offers only the named items; the rest
  stay available (DSC-62). Refs are bare `kind:name`; a prefix in effect for the
  entry (`as`) is applied at install (DSC-63). A ref naming an item the source
  does not offer is an error at meld. `install-items` and `install = true` are
  mutually exclusive on one entry (DSC-64).

### Per-entry prefix (DSC-39)

`as = "<prefix>"` imposes a namespace prefix on a nested source, exactly like
`meld --as`: its items install as `<prefix>-<name>`. The `../namespacing` entry
sets `as = "rev"`, so its `review` skill installs as `rev-review`. `as` is a
registry concern and always applies, independent of the DSC-60 gate below.

### Authoritative pin vs the fallback gate (DSC-59/60/65)

An entry may also carry configuration the nested source would normally declare in
its own `mind.toml`: a pin directive (one of `follow-branch` / `pin-tag` /
`pin-ref`, DSC-41), `roots` (DSC-50), and `[[discover.sources.hooks]]` (HOOK-50).
These split into two categories with different precedence:

* `roots` and `hooks` are fallback-only (DSC-60). They apply only when the nested
  source ships no `mind.toml` of its own. If it has one (even a `mind.toml` that
  does not itself declare roots/hooks), that file is authoritative and the
  curator-supplied `roots`/`hooks` are ignored with a warning.
* the pin directive is authoritative (DSC-65). It sets the nested source's pin
  whether or not that source ships a `mind.toml`, overriding the source's own
  `[source]` pin. It is exempt from the DSC-60 gate; an entry that supplies only
  a pin (no roots/hooks) does not trigger the "ignored" warning. The full
  precedence is: a consumer's direct top-level `meld` pin flag, then the entry's
  pin, then the source's own `[source]` pin, then the default branch.

This is what lets a generated registry reproduce exact revisions: `dump` emits a
per-entry `pin-ref` (see Try it), which is the authoritative pin.

### Recursion (DSC-54/55)

By default, melding a super-source installs only its own items plus the entries
the curator marked `install = true` or `install-items` (DSC-54). The whole chain
is still registered, so unflagged sources are available but not installed. Add
`meld --recursive` (`-r`) to install every nested source regardless of its flag
(DSC-55).

The inline-table and array-of-tables forms are equivalent for plain entries:

```toml
[discover]
sources = [
    { source = "../tooling", install-items = ["skill:scan"] },
]
```

```toml
[[discover.sources]]
source = "../tooling"
install-items = ["skill:scan"]
```

The array-of-tables form is required for an entry that carries
`[[discover.sources.hooks]]`; the inline form cannot express nested tables.

## Layout

```
skills/onboard/SKILL.md   this repo's own convention item (description in frontmatter)
mind.toml                 [source] metadata + [discover].sources curating the chain
```

`mind.toml` lists four nested sources, each demonstrating one knob:

* `../explicit` plain: registered and left available.
* `../namespacing` with `as = "rev"` and `install = true`: all items installed,
  namespaced to `rev-`.
* `../tooling` with `install-items = ["skill:scan"]`: only `skill:scan` installed.
* `../starter` adopt entry: an authoritative `follow-branch` pin plus
  fallback-only `roots`/`hooks` for a source that ships no `mind.toml`
  (DSC-59/60/65).

The nested specs are local paths (`../explicit` and friends) to the sibling
example repos in this tree, so the file is safe to read and copy; a real
super-source lists remote specs (`owner/repo`, `git@host:owner/repo`, or a URL)
in the same positions.

## Try it

The `review` verb validates the `[discover].sources` shape with no network:

```
mind review examples/super-source
```

The nested specs are local paths (`../explicit` and friends) that resolve against
the sibling example repos in this `examples/` tree, so melding this directory
registers those siblings as the chain. The nested `../` specs resolve relative to
the meld's working directory, so run it from inside this directory:

```
cd examples/super-source
mind meld . --yes
```

A bare meld registers all five sources, then installs only the super-source's own
items plus the `install = true` / `install-items` entries. A local-path source
registers as `local/<parent-dir>/<repo>`, so the exact source name depends on the
path; the bare repo names are used below for brevity:

```
+ melded super-source (1 item(s))
+ melded explicit (2 item(s))
+ melded namespacing (4 item(s))
+ melded tooling (2 item(s))
+ melded starter (1 item(s))
melded 5 source(s)
learned skill:team-onboard from super-source
learned skill:rev-review from namespacing
learned agent:rev-dev from namespacing
learned agent:rev-lead from namespacing
learned rule:rev-style from namespacing
learned skill:scan from tooling
note: this source curates other sources; run `mind probe` to browse and search what is available
```

`../explicit` is registered but none of its items are installed (no install
flag), `../namespacing` installs all four items under the `rev-` prefix, and
`../tooling` installs only `skill:scan`. `meld --recursive` (`-r`) would instead
offer every nested source's items for install, including `../explicit`.

Browse what the chain offers without the TUI:

```
mind probe --no-tui
```

`recall --sources` shows the recorded pin/commit for each registered source, the
`as:` prefix where one is in effect, and a `hook` marker on the adopt entry (the
commit hashes vary by clone):

```
*  super-source  ...  [61602d93]
*  explicit      ...  [8e1f53ff]
*  namespacing   ...  [2c427fc4 as:rev]
*  tooling       ...  [b80d1a2d]
*  starter       ...  [f6d428d4 hook]
```

`recall` shows the installed items, including the prefixed names from the
`as = "rev"` entry and the single subset item from `../tooling`:

```
* namespacing  [2c427fc4 as:rev]
  +  agent:rev-dev     installed
  +  agent:rev-lead    installed
  +  rule:rev-style    installed
  +  skill:rev-review  installed
* tooling  [b80d1a2d]
  +  skill:scan   installed
  -  tool:detect  available
```

### dump

`mind dump` writes a super-source `mind.toml` of this shape from the current
installed state, so a setup can be reproduced or shared by melding the output
rather than hand-authoring one:

```
mind dump
```

The emitted file (the `pin-ref` commits are the recorded revisions and vary by
clone; the super-source's own entry and the `../starter` entry are elided here):

```toml
[source]
description = "Generated by `mind dump`. Meld this file to reproduce the recorded source set and install selection."

[[discover.sources]]
source = "../explicit"
pin-ref = "8e1f53ff0043024860a53932b2eeabab438452fe"
install = false

[[discover.sources]]
source = "../namespacing"
as = "rev"
pin-ref = "2c427fc4e9ba2895d13c4f93d65132833c406e34"
install = true

[[discover.sources]]
source = "../tooling"
pin-ref = "b80d1a2dc4f6b5b6f273926663e22567f0c8b094"
install-items = ["skill:scan"]
```

Each entry carries a `pin-ref` of that source's recorded commit. This is the
DSC-65 authoritative pin, so melding the output reproduces each source at the
exact revision. `dump` stamps the install directive from the installed set: all
items installed yields `install = true`, none yields `install = false`, a subset
yields `install-items = [...]` (the same three forms above). It references each
source rather than inlining its inventory: globs, items, and hooks are read from
each source's own `mind.toml` when the output is melded. `mind dump
--whole-sources` instead emits `install = true` for every source regardless of
how many of its items are installed. Use `--output <path>` to write to a file.

### Teardown

Use the source names `recall --sources` reports for your clone (a local path
registers as `local/<parent-dir>/<repo>`):

```
mind forget '<super-source>#*'
mind forget '<namespacing>#*'
mind forget '<tooling>#*'
mind unmeld <super-source>
mind unmeld <explicit>
mind unmeld <namespacing>
mind unmeld <tooling>
mind unmeld <starter>
rm -rf <demo-dir>
```

A nested source the super-source registered is left registered after you unmeld
the super-source; removing it stays an explicit `unmeld` per source.

## Verified

`tests/cli.rs::example_super_source_validates` runs `review` on this directory
and asserts it validates, so the example stays correct as the code changes.

## See also

`../../spec/discovery.md` (DSC-58, DSC-59, DSC-60, DSC-61, DSC-62, DSC-63,
DSC-64, DSC-65: the `[discover].sources` registry, per-entry install control,
the `as` prefix, the authoritative pin, and the fallback gate) and
`../../spec/dump.md` (DUMP-1..8: generating a super-source from installed state).
