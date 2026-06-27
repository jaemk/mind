#!/usr/bin/env bash
set -euo pipefail

# Harmless per-item install hook: runs after the item's store copy and links are
# in place. A real hook might register the item with an external tool here.
echo "explicit-example: scan installed"
