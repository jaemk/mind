#!/usr/bin/env bash
# Entrypoint for the `detect` tool. Resolved by {{tools:detect}}.
set -euo pipefail
dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$dir/lib.sh"
detect_kind "${1:-.}"
