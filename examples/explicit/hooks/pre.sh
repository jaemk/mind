#!/usr/bin/env bash
set -euo pipefail

# Harmless per-item uninstall hook: runs before the item's paths are removed. A
# real hook might deregister the item from an external tool here.
echo "explicit-example: scan removed"
