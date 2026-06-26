---
description: Detect the project type from files in the current directory.
bin: detect.sh
---

A shared helper tool. Skills and agents invoke it via `{{tools:detect}}` and
read its non-entrypoint files via `{{path:tool:detect}}/<file>`. It is store-only:
`learn` copies it into the store but links it into no agent home.
