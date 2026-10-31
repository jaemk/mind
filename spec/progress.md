# Progress

Status: done. A one-line spinner on stderr that names the step in flight
while a slow step runs (a clone, a fetch, a source refresh), so a verb like
`meld` does not sit silent.

## Display

- `PRG-1` A progress step draws one transient line on stderr: a spinner frame,
  a space, and the step's label, redrawn in place while the step runs. When the
  last live step ends the line is erased, so the spinner leaves nothing behind.
  Steps nest: the newest live step's label is drawn, and the enclosing step's
  label returns when it ends. Progress never writes to stdout, and a verb's
  stdout is byte for byte the same with progress on or off.
- `PRG-3` With progress on by detection (PRG-2), a step draws only once it has
  run 200ms, so a fast step never flickers onto the screen. With
  `MIND_PROGRESS=always`, every step draws as soon as it starts.
- `PRG-7` A label is sanitized (`strip_ansi`, DSC-95) before it is drawn, since
  it can carry a source-controlled name or URL, and cut to the terminal width
  (`COLUMNS` when set, else the terminal's, else 80) with a trailing `...`, so
  the line never wraps.
- `PRG-8` The spinner frames are Braille dots on a UTF-8 locale and `|/-\`
  under `--ascii` or a non-UTF-8 locale. The frame is colored only when stderr
  is a terminal and neither `NO_COLOR` nor `--ascii` is set.

## When it is on

- `PRG-2` Progress is off under `--json`, under the global `--no-progress`
  flag, and with `MIND_PROGRESS=never`. `MIND_PROGRESS=always` turns it on
  even when stderr is not a terminal, `TERM` is `dumb`, or `CI` is set.
  Otherwise (`MIND_PROGRESS` unset, empty, or any other value) it is on when
  stderr is a terminal, `TERM` is not `dumb`, and `CI` is unset, empty, `0`,
  or `false`.
- `PRG-9` The spinner is hidden while the `probe` TUI owns the terminal and
  shown again for a verb the TUI runs on the suspended terminal (TUI-44).

## Steps

- `PRG-4` These operations are progress steps:
  - a `git clone`, `git fetch`, or `git ls-remote` that is not already under
    another step, labeled `cloning <url>`, `fetching <url>`, or
    `listing tags of <url>`;
  - `sync` and `upgrade`'s source refresh: one step per source labeled
    `syncing <source>` when sources are fetched one at a time, or one counted
    step `syncing sources (<done>/<total>)` for a parallel refresh (CLI-248),
    under which a worker's git calls draw no label of their own;
  - an item install's staging copy, hash, and reference expansion, labeled
    `installing <kind>:<name>`;
  - the whole-registry catalog scan, labeled `scanning melded sources`, when
    not already under another step.

## Sharing the terminal

- `PRG-5` Output that reaches the terminal while a step may be drawn clears
  the spinner line first and lets it return afterwards, so nothing is written
  onto the end of a spinner line: advisory notes and warnings
  (`render::note`/`warn`, the scan's warnings), the warnings printed during a
  source refresh, every confirmation and free-form prompt, the hook consent
  prompts, and a hook's whole run. No spinner is drawn while a hook runs; its
  framed output is its progress.
- `PRG-6` While progress is on, a git call runs with prompts off
  (`GIT_TERMINAL_PROMPT=0`, ssh `BatchMode=yes`, no connect timeout), so a
  credential, passphrase, or host-key prompt cannot be drawn over. A failure
  that stands in for such a prompt (CLI-248's suppressed-prompt failure) is
  retried once with prompts allowed and the spinner cleared. A call already
  non-interactive (the TUI, a parallel fetch worker, auto-update) is not
  retried.
