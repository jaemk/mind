#!/usr/bin/env bash
set -euo pipefail

# Harmless teardown: remove the markers the install hooks created.
here="$(cd "$(dirname "$0")" && pwd)"
rm -f "$here/.built" "$here/.cache"
echo "hooks-example: removed helper tooling"
