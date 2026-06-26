#!/usr/bin/env bash
set -euo pipefail

# Harmless build step: drop a marker file next to this script so the skill has
# the helper tooling it expects. Passing --cache writes a second marker.
here="$(cd "$(dirname "$0")" && pwd)"
touch "$here/.built"
echo "hooks-example: built helper tooling"

if [ "${1:-}" = "--cache" ]; then
  touch "$here/.cache"
  echo "hooks-example: warmed optional cache"
fi
