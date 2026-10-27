# Curate other repos

A source can list other repos instead of, or as well as, shipping its own
items. Melding it registers every listed repo, optionally installs from them,
and `mind curate` keeps consumers in step when you change the list. Use it for
a team's standard set, a themed collection, or your own dotfiles-style setup.

```toml
# mind.toml
[source]
description = "Acme's agent toolkit"

[discover]
sources = [
  { source = "acme/agents" },                               # registered, nothing installed
  { source = "acme/skills", namespace = "acme", install = true },
  { source = "anthropics/skills", install-items = ["skill:pdf", "skill:docx"] },
  { source = "acme/big-library", profiles = ["basic"] },
]
```

`mind dump` writes this shape from an existing setup, so you can start from
what you already have installed; see [Back up and reproduce your setup](dump.md).

## Entry keys

| key | meaning |
|-----|---------|
| `source` | any repo `meld` accepts: `owner/repo`, a URL, a local path, or an [item link](item-links.md) |
| `namespace` | install that source's items under a namespace, as `meld --namespace` does |
| `install` | `true` offers all its items for install at meld (default `false`) |
| `install-items` | offer only these `kind:name` items (literal names, no globs) |
| `profiles` | enable these [install profiles](authoring-profiles.md) of that source |
| `default-profile` | `false` leaves that source's `default` profile out |
| `follow-branch`, `pin-tag`, `pin-ref`, `pin-version` | pin that source (at most one); `pin-version` takes a release range such as `"^1.2"`, or `"*"` for every tagged release |
| `kind` | for an item link to a `.md` file: `agent`, `rule`, or `command` |

`install = true` cannot be combined with `install-items` or `profiles`.
`install-items` with `profiles` installs the union. An `install = true` entry
whose source declares a `default` profile installs that profile, as a plain
meld would.

The same list can be written as tables:

```toml
[[discover.sources]]
source = "acme/skills"
namespace = "acme"
install = true
```

## Ship your own items too

A bare `[discover].sources` does not turn off convention discovery, so the repo
can ship its own `skills/` and `agents/` alongside the list. Only `[[items]]`
or `[discover]` kind globs make the file authoritative; see
[The mind.toml file](mind-toml.md#when-does-mindtoml-take-over-discovery).

## Configure a repo that has no mind.toml

An entry can supply settings for a listed repo that ships none of its own:

| key | meaning |
|-----|---------|
| `roots` | scan these directories instead of the repo root |
| `add-roots` | scan these directories in addition |
| `flat-skills` | skills sit at the root with no `skills/` directory |
| `[[discover.sources.hooks]]` | hooks to run for that repo, same shape as `[[hooks]]` |

```toml
[[discover.sources]]
source = "owner/unonboarded"
follow-branch = "main"
roots = ["packages/agents"]

[[discover.sources.hooks]]
run = "make build"
name = "build tooling"
```

These four apply only when the listed repo has no `mind.toml` at all. If it
has one, even one that sets none of them, its own file wins and `mind` warns
that yours were ignored. This keeps a curator from reaching past a repo's own
export control. The pin keys, `namespace`, `install`, `install-items`, and
`profiles` always apply.

## How changes reach consumers

When you push a change to the list, consumers run `mind curate` to apply it:
new entries are registered and installed as declared, changed pins re-pin,
curated sources upgrade, and dropped entries are reported (removed only with
`--prune`). `sync` alone registers new entries but installs nothing from them.
See [Follow a curated collection](curate.md).

## A Claude marketplace is a curator too

A `.claude-plugin/marketplace.json` catalog is read the same way, and a repo
can be both a marketplace and a mind curator. See
[Claude plugins and marketplaces](marketplace.md).

Examples:
[super-source](https://github.com/jaemk/mind/tree/main/examples/super-source),
[marketplace-curator](https://github.com/jaemk/mind/tree/main/examples/marketplace-curator).
