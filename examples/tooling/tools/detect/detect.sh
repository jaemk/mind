#!/usr/bin/env bash
# Entrypoint for the `detect` tool. A markdown item reaches it with the
# {{tools:detect}} token (see skills/scan/SKILL.md); that token expands only in
# markdown, not here, so this file finds its own directory itself, below.
set -euo pipefail
dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$dir/lib.sh"
detect_kind "${1:-.}"
