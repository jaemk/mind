# A non-entrypoint file of the `detect` tool. An item reaches it via
# {{path:tool:detect}}/lib.sh rather than the {{tools:detect}} entrypoint.
detect_kind() {
  local root="$1"
  if [ -f "$root/Cargo.toml" ]; then echo rust
  elif [ -f "$root/package.json" ]; then echo node
  elif [ -f "$root/pyproject.toml" ]; then echo python
  else echo unknown
  fi
}
