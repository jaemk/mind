---
name: secret
description: An internal-only skill that should not be offered.
---

# secret

An internal-only skill. It ships in the repo but is not for export.

Its `SKILL.md` would match the `skills` include glob, but the `exclude` glob
`internal/**` drops any path under `internal/` after the include pass (DSC-37),
so it is not in the catalog.
