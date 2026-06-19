---
name: lead
description: Coordinates a change by delegating to its siblings.
---

# lead

Coordinate a change by delegating to sibling items. Each sibling is named with a
reference token so the reference survives a namespace prefix (see the repo
README for how the tokens expand).

Workflow:

1. Delegate the implementation to the {{ns:dev}} agent.
2. Have the {{ns:review}} skill check the result.
3. Apply the {{ns:style}} rule to all output.
