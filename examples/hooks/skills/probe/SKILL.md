---
name: probe
description: Inspect the working tree using the helper tooling the source builds at meld time.
---

# probe

This skill inspects the working tree. It assumes the helper tooling has already
been built, which the source's install hook does at meld time. Without that
build step the skill has nothing to call, so the hook is what makes the skill
usable the moment it is melded.

The unmeld hook removes the same tooling, leaving the checkout as it was before
the meld.
