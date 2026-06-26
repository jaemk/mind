# Tooling example

A source that ships a shared `tool` and a skill that references it through
path-reference tokens.

## What it shows

- The `tool` item kind: `tools/detect/` is a store-only installable. `learn`
  copies it into the store but links it into no agent home; other items reach it
  by reference. Its `TOOL.md` supplies a `description` and `bin`.
- `{{tools:detect}}` expands to the tool's entrypoint (its resolved `bin`,
  `detect.sh`).
- `{{path:tool:detect}}` expands to the tool's store directory, for a
  non-entrypoint file (`{{path:tool:detect}}/lib.sh`).
- `{{self}}` expands to an item's own store directory, so the `scan` skill
  addresses its bundled `resources/notes.md` without hardcoding its installed name.

All three tokens expand at install (like `{{ns:}}`) to a path under
`~/.mind/store`, so a reference stays correct under a namespace prefix and with
several agent homes configured.

## Layout

```
tools/detect/TOOL.md            description + bin: detect.sh
tools/detect/detect.sh          the entrypoint ({{tools:detect}})
tools/detect/lib.sh             a non-entrypoint file ({{path:tool:detect}}/lib.sh)
skills/scan/SKILL.md            references {{tools:detect}}, {{path:tool:detect}}, {{self}}
skills/scan/resources/notes.md  bundled, addressed as {{self}}/resources/notes.md
mind.toml                       [source] description only (convention scanning stays on)
```

## Try it

```
cp -r examples/tooling /tmp/tooling-demo
cd /tmp/tooling-demo && git init -q && git add -A && git commit -qm init
mind meld /tmp/tooling-demo

# A path-token reference is not an install dependency (only {{ns:}} is), so learn
# the tool alongside the skill. The tokens then expand to store paths.
mind learn detect
mind learn scan
cat ~/.mind/store/skill/scan/SKILL.md     # {{tools:detect}} -> ~/.mind/store/tool/detect/detect.sh

# The tool is store-only: it is in the store but linked into no agent home.
mind recall detect
```

## Verified

`tests/cli.rs::example_tooling_expands_path_tokens` melds this directory and
asserts the token expansion, so the example stays correct as the code changes.
