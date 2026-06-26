---
description: Scan a project and report its type using a shared helper tool.
---

# scan

Determine a project's type and note it.

1. Run the shared tool on the target directory: `{{tools:detect}} .`
2. For a finer check, source its library directly and call a single function:
   `. {{path:tool:detect}}/lib.sh`
3. Record the result in this skill's own notes file: `{{self}}/resources/notes.md`

`{{tools:detect}}` resolves to the tool's entrypoint, `{{path:tool:detect}}`
to the tool's store directory (for non-entrypoint files), and `{{self}}` to this
skill's own store directory. All three are expanded at install and stay correct
under a namespace prefix or with several agent homes configured.
