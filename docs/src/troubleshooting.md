# Troubleshooting

Find the symptom below. `mind introspect` is a good first step for anything
installed: it reports missing links, drift, and unsynced sources.

## Installing

### An item did not show up in `~/.claude`

Run `mind introspect`. It reports missing links and drift, and
`mind introspect --fix` recreates missing links. If the item links into a
non-Claude lobe only, check that lobe's kinds filter (next entry).

### A skill links into Gemini or Codex but a rule, command, or workflow does not

Presets link skills only. Rules, commands, and workflows have no directory
equivalent in those harnesses, so they stay in `~/.claude`. See
[Which kinds link where](lobes.md#which-kinds-link-where).

### `learn` refuses with `LinkOccupied`

`mind` never overwrites a file or link it did not create. Move the existing
entry aside and `learn` again, pass `learn --force` to replace it, or
[absorb](unmanaged.md) it if it is something you want to keep under version
control.

### Two sources ship an item with the same name

Namespace one: `mind meld <repo> --namespace <prefix>` installs its items as
`<prefix>:<name>`. Agents are different, since the harness keys them by
frontmatter name. See [Fix name collisions](collisions.md).

### An install is refused as an unsupported platform

`mind` links with real symlinks and supports unix-like systems only. On
Windows, run it under WSL.

### `learn` asks to install a tool or resource I did not select

The item you chose names it with `{{tools:name}}`, `{{resources:name}}`, or
`{{path:...}}`, and those tokens are dependencies, so `learn` offers the whole
set. Confirm with `--yes`. Without the tool or resource, the item's expanded
path would point at nothing. See [Items that depend on other
items](installing.md#items-that-depend-on-other-items).

### An agent's tool permissions do not work in Gemini or Codex

`mind` links files as they are and does not translate frontmatter. Keys such as
Claude's `tools:` allow-list may be ignored by another harness. See
[Frontmatter is not translated](lobes.md#frontmatter-is-not-translated).

## Updating and versions

### An item shows as out of date with no upstream change

Its content no longer matches the source, usually because a store or source
file was edited by hand. `sync` then `upgrade` the item, or restore the file.

### Roll back to an older version

Re-pin the source to the older commit, then upgrade the item:

```
mind meld <repo> --pin <old-sha>
mind upgrade <item> --yes
```

See [Pin a source to a version](pinning.md#roll-back-an-item).

### `upgrade` did nothing after a curator changed its list

`sync` and `upgrade` do not apply a curator's decisions. Run `mind curate`. See
[Follow a curated collection](curate.md).

## Sources

### A melded local source was moved or deleted

Listing commands warn and continue. Find its name with
`mind recall --sources`, then `mind unmeld <name>`, or restore the directory.

### A source's items disappeared after setting `roots`

`roots = []` scans nothing. Omit `roots` to scan the repo root, or list the
real subdirectories.

### A `mind.toml` change fails to parse

The file is strict, so a misspelled key is an error. Common slips: pin keys are
hyphenated (`follow-branch`, `pin-tag`, `pin-ref`), as is `min-mind-version`.
Run `mind review <path>` to check the file. See [The mind.toml file](mind-toml.md).

### A file under `resources/` is not offered

Without an authoritative `mind.toml`, a resource is offered only when an item
in the same source references it with `{{resources:name}}`. A plain mention of
`resources/<name>` in prose does not count. `mind review` lists each skipped
entry as `unreferenced-resource`. See [Share data files](resources.md).

### A meld finds no items

The repo does not use the conventional layout. The meld prints the paths it
scanned and the flags that reach other layouts (`--add-root`, `--root`,
`--flat-skills`). See [Source layout and item kinds](source-layout.md).

## Network and authentication

### `meld` or `sync` says "git executable not found"

`mind` runs `git` for every clone and fetch. Install it and put it on `PATH`.

### A private repo fails to authenticate

`mind` uses your git setup. Configure a git credential helper for HTTPS, or
meld the SSH form (`git@host:owner/repo`, or `ssh = true` in
`~/.mind/config.toml`) with a key in `ssh-agent`. See
[Restricted networks](enterprise.md#private-repos).

### `meld` or `sync` fails behind a proxy

Set `HTTPS_PROXY` (and `NO_PROXY` for internal hosts), or git's `http.proxy`:

```
git config --global http.proxy http://proxy.corp:8080
```

See [Proxies](enterprise.md#proxies).

### A TLS or certificate error behind a company proxy

Install the corporate CA in the system trust store; that fixes every case.
Otherwise point git at it with `http.sslCAInfo` and `evolve` with
`SSL_CERT_FILE`. See [Custom CA](enterprise.md#custom-ca--tls-intercepting-proxy).

### `evolve` fails with a 403 from `api.github.com`

That is GitHub's unauthenticated rate limit, often hit behind a shared egress
IP. Set `GH_TOKEN` (or `GITHUB_TOKEN`) to any GitHub token, or skip the lookup
with `mind evolve --to <version>`.

## Agent homes

### A Gemini lobe at `~/.gemini` stopped working

The `gemini` preset path is `~/.gemini/config`. Replace an older entry:

```
mind config lobes remove ~/.gemini
mind config lobes add --preset gemini
```

### A lobe points at a directory that does not exist

Adding a lobe creates its directory and links installed items into it. For an
older config whose lobe is unreachable, `mind introspect --fix` prunes it.

## Where things live

`~/.mind` holds the config, store, and source clones; see
[Paths](configuration.md#paths). Override the roots with `MIND_HOME` and
`MIND_DEFAULT_LOBE`.

## The terminal UI will not exit

Press `q` in the main view, or Ctrl-C twice from anywhere.
