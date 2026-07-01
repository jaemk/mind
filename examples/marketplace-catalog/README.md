# marketplace-catalog

A marketplace catalog fixture for testing `mind`'s Claude plugin marketplace support (MKT-7..8).

Layout:
- `.claude-plugin/marketplace.json` - lists two in-repo plugins by relative path
- `plugins/alpha/` - a plugin named `alpha` with one skill (`alpha:one`)
- `plugins/beta/` - a plugin named `beta` with one agent (`two`, bare frontmatter name per NS-40)

The catalog is self-contained: all plugins are in-repo relative paths, no network access needed.
External-plugin cases are built inline in tests using `Sandbox::bare` + `write_and_commit`.

Used by `Sandbox::from_example("marketplace-catalog")` in `tests/cli.rs`.
