---
name: alpha
description: Run the alpha package checks and report the result.
---

# alpha

Run the alpha package's checks, then report pass or fail.

This skill sits at `packages/a/skills/alpha/`, a position neither plain
convention nor `[source].roots` captures cleanly. It is discovered because the
`skills` include glob ends at its `SKILL.md` and the item is its parent
directory (DSC-33).
