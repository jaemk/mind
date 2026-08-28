# Contributing

## Setup

```
cargo build
cargo test
```

`make ci-local` (alias `make check`) is the local gate: `cargo fmt` (in place)
+ `cargo clippy --all-targets --all-features -- -D warnings` + `cargo test
--all-features` + `cargo audit`. Run it before opening a PR; CI runs the
read-only variant (`make ci`: `fmt --check` instead of `fmt`).

The audit step scans the locked dependency tree against the RustSec advisory
database. It is skipped, with a note on stderr, when `cargo-audit` is not
installed (`cargo install cargo-audit --locked`), so a passing gate does not on
its own mean a scan ran. It is the only step that reaches the network, and the
only one whose result can change without the tree changing, so
`make ci-local SKIP_AUDIT=1` opts out when you are offline or an unrelated
upstream advisory is blocking you. CI scans every push and every tag regardless
and does not read that variable.

`make build-release` builds the binary the release workflow ships, with a
dependency SBOM embedded by `cargo auditable` (install the pinned version:
`cargo install cargo-auditable --version 0.7.5 --locked`). `cargo audit bin
target/release/mind` then scans the built artifact without its source.

## The spec

[spec/](spec/) is the normative behavioral spec, and [spec/README.md](spec/README.md)
is its index and the "Feature status" table. Read CLAUDE.md's "Spec is
mandatory for features" section before adding or changing behavior. In short:

- Every spec statement has a stable ID (e.g. `LIFE-4`). Tests cite the IDs
  they cover in a `// spec: ID` comment.
- `tests/spec_coverage.rs` is a coverage gate: it fails if a spec ID is
  neither cited by a test nor listed in its `ALLOWLIST` (with a reason), and
  if a test cites an ID the spec does not define. `make ci`/`make ci-local`
  runs it.
- Only a citation in TEST code counts: everything under `tests/`, and the
  `#[cfg(test)]` regions of `src/`. A `// spec:` comment on production code is
  useful to a reader and you are welcome to write one, but it asserts nothing,
  so the gate ignores it. If a behavior genuinely cannot be exercised
  headlessly, put the ID in the `ALLOWLIST` with the real reason instead of
  leaving a comment on the implementation.
- A feature addition is not complete until spec/ documents it (new IDs) and
  the feature-status row in spec/README.md reflects reality (`done` only when
  implemented and tested), in the same change as the code.

## Pull requests

- Keep the change scoped to what the PR describes.
- Add or update the spec and its feature-status row alongside behavior
  changes, not in a follow-up.
- Add a test that fails without your change and passes with it.
- Run `make ci-local` before opening the PR.
