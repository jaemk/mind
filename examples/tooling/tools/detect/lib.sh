# A non-entrypoint file of the `detect` tool. A markdown item reaches it with
# the {{path:tool:detect}}/lib.sh token rather than the {{tools:detect}}
# entrypoint; that token expands only in markdown, not here.
detect_kind() {
  local root="$1"
  if [ -f "$root/Cargo.toml" ]; then echo rust
  elif [ -f "$root/package.json" ]; then echo node
  elif [ -f "$root/pyproject.toml" ]; then echo python
  else echo unknown
  fi
}
