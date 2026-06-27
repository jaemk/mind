# Unmanaged items

An agent home (lobe) often holds skills, agents, and rules that `mind` did not
install: files written by hand or placed by another tool. `mind` surfaces these
as *unmanaged* items so the listing reflects everything the agent actually sees,
and lets `forget` remove one after a distinct confirmation.

Tools are never linked into a lobe, so unmanaged detection covers skills, agents,
and rules only (spec UNM-1).

## What counts as unmanaged

A lobe entry is unmanaged when its path is not recorded in any manifest item's
`links` (spec UNM-1). A managed symlink and an unmanaged file cannot occupy the
same path simultaneously; a given path is always one or the other.

`mind` scans every configured lobe. An item present in more than one lobe is one
logical item that records each occupied path.

## Listing unmanaged items

### recall

`mind recall` (no arguments) lists unmanaged items after the melded sources,
in a group labeled "unmanaged: not installed by mind", one row per item showing
`kind:name` and the lobe path(s) it occupies (spec UNM-2). No flag is needed to
see them; `--kind` filters them the same way it filters managed items; `--source`
excludes them because they have no source.

`recall --json` covers managed sources only (its schema is unchanged); unmanaged
items are exposed machine-readably through `probe --json` (spec UNM-2, UNM-3).

### probe

`mind probe` includes unmanaged items in its listing and substring search
(spec UNM-3). In the non-interactive listing each unmanaged row is labeled. In
`--json` output each carries `"unmanaged": true` with no `source` or `hash` field.
`--kind` filters; `--source` excludes them.

In the interactive TUI, unmanaged items appear under an **Unmanaged** group node,
browsable and searchable like a source's items (spec UNM-6). See
[Interactive TUI](tui.md).

## Forgetting an unmanaged item

```
mind forget <kind:name>
```

When the ref resolves to an unmanaged item, `forget` removes the lobe entry
itself (the file or directory the user owns) and leaves the manifest unchanged
(spec UNM-4).

**Exact ref only.** An unmanaged item is matched only by its exact `kind:name`.
A glob such as `forget '*'` removes managed items only and never deletes a user's
own files (spec UNM-4).

**Confirmation required.** Every unmanaged removal prompts first, regardless of
count. The prompt states explicitly that the item is not managed by `mind` and
that removal deletes the user's own file or directory, not a symlink (spec UNM-5).

- `--yes` proceeds after displaying that statement.
- A non-TTY run without `--yes` refuses with `ConfirmationRequired` and removes
  nothing (spec UNM-5).

The `--force` / clobber flags do not apply here; nothing is being overwritten.

When a bare name matches both a managed and an unmanaged item, add a kind prefix
to disambiguate (for example `skill:review` vs `agent:review`). See
[Commands](commands.md) for the full `forget` verb reference.

## Planned

`forget --unmanaged [glob]` for bulk removal of unmanaged items is planned but
not yet available (spec UNM-7, UNM-8).
