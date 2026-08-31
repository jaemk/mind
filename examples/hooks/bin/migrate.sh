#!/usr/bin/env bash
set -euo pipefail

# One-shot migration step for the update event: it only runs on `mind upgrade`,
# once this source has already been melded and moves to a new commit, never on
# the first meld (HOOK-121).
here="$(cd "$(dirname "$0")" && pwd)"
touch "$here/.migrated"
echo "hooks-example: migrated helper tooling"
