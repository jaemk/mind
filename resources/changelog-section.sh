#!/usr/bin/env bash
# Print the CHANGELOG section body for one version, with surrounding blank lines
# trimmed. Used by the release workflow to populate the GitHub release notes.
#
# Usage: changelog-section.sh VERSION [CHANGELOG_FILE]
#   VERSION         version without a leading 'v' (e.g. 0.6.0)
#   CHANGELOG_FILE  defaults to CHANGELOG.md
#
# Emits the lines between the `## [VERSION]` heading and the next `## [` heading.
# Exits non-zero if the version section is not found.
set -euo pipefail

version="${1:?usage: changelog-section.sh VERSION [CHANGELOG_FILE]}"
file="${2:-CHANGELOG.md}"

awk -v ver="$version" '
  index($0, "## [" ver "]") == 1 { grab = 1; found = 1; next }
  grab && index($0, "## [") == 1 { exit }
  grab { lines[n++] = $0 }
  END {
    if (!found) { exit 1 }
    s = 0;   while (s < n   && lines[s] ~ /^[[:space:]]*$/) s++
    e = n-1; while (e >= s  && lines[e] ~ /^[[:space:]]*$/) e--
    for (i = s; i <= e; i++) print lines[i]
  }
' "$file"
