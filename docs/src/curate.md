# Follow a curated collection

A *curator* is a source that lists other sources: a `mind.toml` with
`[discover].sources` (a super-source), or a Claude plugin marketplace catalog.
Melding one registers everything it lists in one step, and `mind curate`
keeps you in step as the list changes.

```
mind meld acme/agent-registry        # registers every source it lists
mind curate                          # later: apply what the curator changed
```

## Meld a curator

A meld registers each listed source as its own source, with its own clone and
pin. By default it installs only the curator's own items plus the entries the
curator marked for install (`install = true`, `install-items`, or `profiles`).
Everything else stays registered and available to `learn` or `probe`.

```
mind meld acme/agent-registry --recursive   # also offer every listed source's items
mind probe --source acme                    # browse what the chain offers
```

`mind probe`'s Available group also suggests sources listed by your curators
that you have not melded yet; see
[Browse in the terminal UI](tui.md#registry-preview).

## Apply a curator's changes

`sync` and `upgrade` do not act on a curator's decisions. `sync` registers a
newly listed source but installs nothing from it, and neither re-reads a
curator's pins or notices an entry it dropped. `curate` does:

```
mind curate              # report the plan, ask once, apply
mind curate --check      # report only
mind curate --yes        # apply without asking
mind curate --no-sync    # plan against the clones on disk; fetch nothing
```

It fetches every curator and the sources it manages, then reports one plan:

| change | meaning | applying it |
|--------|---------|-------------|
| `register` | the curator lists a source you do not have | registers it, then installs what the entry declares |
| `install` | the entry declares items or profiles that are not installed | installs them |
| `repin` | the entry's pin no longer matches the source's recorded pin | re-pins and checks out the new commit |
| `upgrade` | a curated source's installed items are out of date | runs the upgrade for those sources |
| `unlist` | a source you registered from the list is no longer on it | `--prune` only: uninstalls its items and drops it |
| `namespace` | the entry declares a different namespace than your instance has | report only; prints the `unmeld` and `meld` that adopt it |
| `adopt` | the curator lists a source you already have but does not manage | report only; run the `mind curate --adopt <identity>` it prints |

Changes apply in the order of the table. `unlist` needs `--prune`, so an
unattended `curate --yes` never uninstalls anything. `namespace` and `adopt` are
never applied by `curate`, `--yes` included.

## What curate will not touch

`curate` changes only sources a curator registered. A source you melded
yourself, even one a curator also lists, shows up as `adopt` instead of being
changed. Run the printed `--adopt` command to hand it to the curator; after
that it is managed like any curated source.

A failure on one source (an unreadable curator, a bad pin, a scan error) is
listed under `skipped` and does not stop the run. A non-interactive run without
`--yes` reports the plan and applies nothing. Registering and installing run
the same hook prompts as `meld` and `learn`, so
`--accept-hooks` works on `curate` too. Pending
changes exit 0; under `--json`, `outcome` is `clean`, `pending`, or `applied`.

## Make your own

To build a curated list for yourself or a team, see
[Curate other repos](curating.md) and
[Share a setup with your team](team-setup.md). `mind dump` writes one from
your current setup; see [Back up and reproduce your setup](dump.md).

Spec: [spec/curate.md](https://github.com/jaemk/mind/blob/main/spec/curate.md).
