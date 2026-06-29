# Absorb

Status: done. An agent home often holds skills, agents, and rules that `mind`
did not install: files a user wrote by hand, or items another installer dropped
in (unmanaged.md UNM-1). `forget --unmanaged` (UNM-7) deletes them; `absorb` is
the constructive inverse. It claims one unmanaged item into a version-controlled
source the user owns and installs it through the normal managed path, so a
hand-written item becomes a first-class `mind` item that syncs, evolves, and
forgets like any other. `absorb` never copies an item into the store as a
sourceless orphan: the file is moved into a real source so the user properly
manages their personal items, and the lobe path is reoccupied by a managed link.

"item", "source", "store", "link", and "lobe" are as in [README.md](README.md).
Unmanaged items, their detection, and single-ref resolution are defined in
[unmanaged.md](unmanaged.md). The user config file (`config.toml`) is defined in
[storage.md](storage.md).

## What absorb does

- `ABS-1` `mind absorb <ref>` resolves `<ref>` to a single unmanaged item by the
  UNM-4 rules (an exact `kind:name`, a kind prefix disambiguates, a glob is
  rejected, a source-qualified ref never matches). It moves that item's lobe entry
  into the destination source at the convention path for its kind (`skills/<name>/`,
  `agents/<name>.md`, `rules/<name>.md`, relative to the source root or its first
  scan root, DSC-50), commits it (ABS-5), melds the source if it is not yet
  registered, and `learn`s the item. The lobe path is then occupied by a managed
  link, not the user's own file. Only skills, agents, and rules can be absorbed,
  since tools are never linked into a lobe (TOOL-3) and so are never unmanaged.
  A glob ref is an error (`InvalidItemRef`); bulk absorb is not offered.
- `ABS-8` After absorb the item is an ordinary managed item: it is recorded in the
  manifest keyed `kind:effective-name` with the destination source as its source
  and its file registry (storage.md), and it participates in `sync`, `evolve`,
  `upgrade`, and `forget` like any installed item. Its effective name follows the
  destination source's prefix (`as` or `[source].prefix`, namespacing.md) when one
  is in effect.

## Choosing the destination

- `ABS-2` The destination source is resolved from, in precedence order, the
  `--to <path>` flag, the `MIND_ABSORB_TO` environment variable, then the
  `absorb_to` key in `config.toml`. The first one set wins; a later source is not
  consulted.
- `ABS-3` When none of the three (ABS-2) is set and the run is interactive,
  `absorb` prompts for a destination and offers the built-in `~/.mind/personal`,
  creating that directory and initializing a git repository in it if it does not
  exist. A non-interactive run (non-TTY) with no destination configured is an
  error (`ConfirmationRequired`) and changes nothing, since there is no safe
  default to assume silently.
- `ABS-4` When the destination was resolved interactively (ABS-3), and only then,
  `absorb` offers to save the chosen path as `absorb_to` in `config.toml`. On yes
  it writes the key (creating the config if absent, STO-15); on no it leaves the
  config unchanged. A destination supplied by `--to`, `MIND_ABSORB_TO`, or an
  existing `absorb_to` is used as-is and prompts no save.
- `ABS-9` `absorb`'s help text and documentation state the three ways to set the
  destination and their precedence (`--to` over `MIND_ABSORB_TO` over the
  `config.toml` `absorb_to` key).

## The move

- `ABS-5` The destination must be a git repository so the moved item sits at a
  resolvable commit for meld, pin, and learn. The built-in `~/.mind/personal` is
  created and initialized on demand (ABS-3); a `--to` / `MIND_ABSORB_TO` /
  `absorb_to` path that is not a git repository is an error. After moving the item
  in, `absorb` stages and commits it in the destination repo with a default
  message (`absorb <kind>:<name>`) so the source has a clean recorded commit before
  it is melded or synced.
- `ABS-6` When the destination source already contains an item at the same
  convention path (`kind:name` collision), `absorb` errors and changes nothing,
  unless `--force` (`-f`) is given, which overwrites the destination path. A bare
  absorb never silently clobbers existing source content.
- `ABS-7` An unmanaged item may occupy more than one lobe (UNM-1). `absorb` takes
  the content from one occupied path; because `learn` relinks the item into every
  configured lobe (STO-14), `absorb` removes the item's other unmanaged copies so
  the managed link can take their place. Because this deletes the user's own files,
  `absorb` lists what it will move and which stray copies it will delete and
  prompts `[y/N]` once before acting. `--yes` (`-y`) skips the prompt; a non-TTY
  run without `--yes` refuses with `ConfirmationRequired` and changes nothing. On
  decline nothing is moved or deleted.
- `ABS-10` `absorb` is transactional in spirit: a failure before the `learn`
  completes (a bad destination, a collision without `--force`, a declined prompt,
  a commit or meld error) leaves the original lobe entry in place and the manifest
  unchanged, so a failed absorb never loses the user's file.
- `ABS-11` Under `--json`, `absorb --yes` emits exactly one structured result
  object (CLI-153) with `action: "absorb"`, `target` set to the resolved item
  ref, `outcome: "absorbed"`, and a `key` field set to the effective `kind:name`
  the item is now managed under. Without `--yes`, `absorb --json` refuses with
  `ConfirmationRequired` (json mode is non-interactive, ABS-7) and writes
  nothing to stdout.
