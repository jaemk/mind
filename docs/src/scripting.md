# Scripts and CI

Every verb runs without a terminal. This page covers prompts, hooks, JSON
output, and exit codes for scripts and CI jobs.

```
mind meld git@github.example.com:platform/agent-config --yes
mind recall --json
```

## Prompts

Pass `--yes` (`-y`) to answer every confirmation. Without it, a command that
would ask on a terminal fails with `ConfirmationRequired` when stdin is not a
terminal, rather than hanging.

`meld` is the exception: without `--yes` and without a terminal, it registers
the source, prints that nothing was installed, and exits 0. Pass `--yes` to
install.

`--yes` does not suppress the notice that names every source a multi-source
`upgrade` or `sync --upgrade` will run hooks for.

## Hooks

Install, update, uninstall, and build hooks are skipped without a terminal,
with a note. `--accept-hooks` runs them unattended on every verb that runs
hooks (`meld`, `learn`, `forget`, `unmeld`, `upgrade`, `sync --upgrade`,
`curate`, `hooks run`). Bare, it accepts every hook; with a list, only those
events:

```sh
mind upgrade --yes --accept-hooks                   # every hook
mind upgrade --yes --accept-hooks=install,update    # not build hooks
mind forget scan --accept-hooks=uninstall
```

The events are `install`, `update`, `uninstall`, and `build`. Any hook whose
event is not listed is skipped as usual. This runs code from the source
without asking, so use it only for sources you trust. See
[Run setup with hooks](install-hooks.md).

Under `--json`, a run that considered any hook adds a `hooks` array to its
document, one result per hook in the order they were considered. A failed run
adds it to the error envelope, with the failing hook last:

```json
{"schema": 1, "error": {"kind": "hook-failed", "message": "..."},
 "hooks": [{"at": "2026-10-28T14:03:09Z", "run": "20261028T140301Z-4242",
            "verb": "meld", "event": "install", "label": "setup",
            "source": "github.com/owner/repo", "command": "make setup",
            "commit": "<sha>", "outcome": "failed", "exit_code": 2,
            "signal": null, "duration_ms": 840, "log": null}]}
```

`outcome` is `ok`, `failed`, `skipped`, `declined`, `aborted`, or
`interrupted`. An item hook also carries `item` (`kind:name`), and a
consumer's `--install-hook` or `--uninstall-hook` override carries
`replaced`, the declared commands it ran in place of. `exit_code`, `signal`,
and `duration_ms` are `null` for a hook that did not run. Pass `--hook-log` to
save each hook's output; `log` then names the file. The same records are kept
in `~/.mind/hooks/history.jsonl` for `mind hooks history`. See
[Hook results and logs](install-hooks.md#hook-results-and-logs).

A run that stops on a failed hook exits 3, and one stopped by a Ctrl-C during a
hook exits 130, so a script can tell them from other errors without `--json`.

To keep logs only for hooks that fail, which suits CI:

```sh
mind upgrade --yes --accept-hooks --hook-log-keep failed
mind hooks history --since 1d --failed --show
```

## JSON output

`--json` is a global flag, accepted before or after the verb. Every verb answers
it with exactly one JSON document on stdout, except:

- `dump`, which always writes TOML.
- `completions` and `man`, which print a script or a roff page.
- `init-source`, which has no result document.
- `evolve`, which writes its own JSON document by a separate path.

Shapes:

- `recall` and `probe`: `{"schema": 1, "items": [...]}`.
- `introspect`: `{"schema": 1, "issues": [...], "sources": N, "items": N}`.
- Every changing verb (`meld`, `learn`, `forget`, `sync`, `upgrade`, `unmeld`,
  `config lobes add` and `remove`) returns an object with `"schema": 1` and at
  least `action`, `target`, and `outcome`.
- `review`: `{"schema": 1, "action": "review", "outcome": "clean|advisory|failed",
  "hard": [...], "advisory": [...], "fixed": [...]}`, each finding being
  `{"kind": "<slug>", "message": "<text>"}`.
- `hooks list`: each matched source's or item's hooks, each with its `last`
  result when there is one. `hooks run`: a tally,
  `{"schema": 1, "action": "hooks-run", "target": "...", "event": "...",
  "existed": N, "ran": N, "skipped": N}`.
- `hooks history`: `{"schema": 1, "action": "hooks-history", "records": [...]}`;
  with `--show`, each record that has a log also carries `output`, its text
  (`null` when the file is gone).
- Any verb that considered a hook: a `hooks` array of results (see
  [Hooks](#hooks)).
- `curate`: one document whose `outcome` is `clean`, `pending`, or `applied`.
- `auto-update run`: `{"schema": 1, "action": "auto-update run", "target": "*",
  "outcome": "ok|partial|skipped", "moved": [...], "held": [...], "failed":
  [...], "breaking": [...]}`.

`config auto-update on|off` returns the changing-verb object with `count` set
to the number of `settings.json` files changed.

`learn --json` refuses with `confirmation-required` when the dependency closure
adds items you did not name, unless `--yes` is given, since there is no prompt
to answer.

### Errors

Under `--json`, an error is a JSON envelope on stdout instead of text on
stderr:

```json
{"schema": 1, "error": {"kind": "item-not-found", "message": "..."}}
```

`kind` is a stable kebab-case slug (`item-not-found`, `source-not-found`, `git`,
`digest-mismatch`, ...) a script can branch on. An optional `details` member
carries more; a failed `review --json` puts its findings document there.
Command-line usage errors (exit 2) stay plain text.

## Exit status

| exit | meaning |
|------|---------|
| 0 | success, including "nothing to do" and `curate` with pending changes |
| 1 | any other runtime error |
| 2 | a command-line usage error |
| 3 | a hook failed (`hook-failed`) |
| 130 | a Ctrl-C interrupted a hook (`hook-interrupted`) |

Specific cases:

- `sync` exits 1 if any source failed to fetch, after saving the ones that
  succeeded.
- `review` exits 1 on a hard finding and 0 on advisory findings only.
- `upgrade <filter>` with no match reports up to date and exits 0;
  `sync <filter>` with no match is an error.

## Output format

Color and Unicode glyphs are used only when stdout is a terminal, the locale is
UTF-8, `NO_COLOR` is unset, and neither `--json` nor `--ascii` is given. Any
one of those failing gives plain ASCII with no escapes. The ASCII forms carry
the same information (`+` installed, `-` available, `!` warning, `x` error).

Other global flags: `--verbose` (`-v`) adds advisory output such as
unguarded-reference warnings, and `--max-metadata-size <SIZE>` sets the
metadata size cap for one run (see
[Configuration](configuration.md#metadata-size-cap)).

## A CI job

```
ssh-keyscan -H github.example.com >> ~/.ssh/known_hosts   # first SSH connection only
mind meld git@github.example.com:platform/agent-config --yes
mind recall --json | jq '.items[].items[] | select(.installed | not)'
```

The last line lists items that are offered but not installed; fail the job if
it prints anything you expected. For a whole-team setup, see
[Share a setup with your team](team-setup.md); for a managed fleet,
[Managed policy](policy.md#auto-meld-org-provisioning).
