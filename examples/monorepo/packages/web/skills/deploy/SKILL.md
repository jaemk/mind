---
name: deploy
description: Build the web package and ship it to the configured environment.
---

# deploy

Build the web package, run its smoke checks, then publish the artifact to the
target environment. Confirm the health endpoint returns OK before reporting
success.

This item lives under `packages/web/`, not the repo root. It is discovered
because `[source].roots` points convention scanning at that subtree. It installs
as `deploy` (its bare name); roots affects where discovery looks, not the
installed name.
