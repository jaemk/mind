# Tooling example

Requires mind >= 0.7. (`min-mind-version`: an older binary warns and asks before melding the source.)

A source that ships a shared `tool`, a shared `resource`, and a skill that
references both through path-reference tokens.

## What it shows

- The `tool` item kind: `tools/detect/` is a store-only installable. `learn`
  copies it into the store but links it into no agent home; other items reach it
  by reference. Its `TOOL.md` supplies a `description` and `bin`.
- `{{tools:detect}}` expands to the tool's entrypoint (its resolved `bin`,
  `detect.sh`).
- `{{path:tool:detect}}` expands to the tool's store directory, for a
  non-entrypoint file (`{{path:tool:detect}}/lib.sh`).
- The `resource` item kind: `resources/markers.json` is a store-only data file.
  `{{resources:markers.json}}` expands to its store path. Convention discovery
  offers it because the skill references it.
- `{{self}}` expands to an item's own store directory, so the `scan` skill
  addresses its bundled `resources/notes.md` without hardcoding its installed name.
- Path tokens are dependencies: `mind learn scan` installs `detect` and
  `markers.json` with the skill.

The tokens expand at install (like `{{ns:}}`) to a path under
`~/.mind/store`, so a reference stays correct under a namespace prefix and with
several agent homes configured.

## Layout

```
tools/detect/TOOL.md            description + bin: detect.sh
tools/detect/detect.sh          the entrypoint ({{tools:detect}})
tools/detect/lib.sh             a non-entrypoint file ({{path:tool:detect}}/lib.sh)
resources/markers.json          a shared data file ({{resources:markers.json}})
skills/scan/SKILL.md            references {{tools:detect}}, {{path:tool:detect}},
                                {{resources:markers.json}}, {{self}}
skills/scan/resources/notes.md  bundled, addressed as {{self}}/resources/notes.md
mind.toml                       [source] metadata only (convention scanning stays on)
```

## Try it

```
cp -r examples/tooling /tmp/tooling-demo
cd /tmp/tooling-demo && git init -q && git add -A && git commit -qm init
mind meld /tmp/tooling-demo

# The skill's path tokens are dependencies, so learning it installs the tool
# and the resource too (after showing the dependency tree).
mind learn scan --yes
cat ~/.mind/store/skill/scan/SKILL.md     # {{tools:detect}} -> ~/.mind/store/tool/detect/detect.sh

# The tool and resource are store-only: in the store, linked into no agent home.
mind recall detect
ls ~/.mind/store/resource/                # markers.json
ls ~/.claude/tools/ 2>/dev/null           # no entry for detect; tools are store-only (TOOL-3)

# Search installed items.
mind probe --no-tui detect

# Teardown (inverse order).
mind forget scan
mind forget detect
mind forget resource:markers.json
mind unmeld tooling-demo
rm -rf /tmp/tooling-demo
```

## See also

- `../../spec/tooling.md` - normative spec for the `tool` kind and path-reference tokens
  (TOOL-1..7 store-only behavior, TOOL-10..16 path tokens).
- `../../spec/resources.md` - the `resource` kind and `{{resources:name}}`.

## Verified

`tests/cli.rs::example_tooling_expands_path_tokens` melds this directory and
asserts the token expansion, so the example stays correct as the code changes.
