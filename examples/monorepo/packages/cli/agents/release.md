---
name: release
description: Cut a release of the cli package: bump the version, tag, and publish.
---

# release

Bump the cli package version, update its changelog, tag the commit, and publish
the build. Verify the published version resolves before reporting success.

This item lives under `packages/cli/`, a second subtree listed in
`[source].roots`. Convention scanning runs under each root independently, so this
agent and the web skill are both discovered from their own packages.
