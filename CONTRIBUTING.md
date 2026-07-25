# Contributing

## Setup

```
cargo build
cargo test
```

`make ci-local` (alias `make check`) is the local gate: `cargo fmt` (in place)
+ `cargo clippy --all-targets --all-features -- -D warnings` + `cargo test
--all-features`. Run it before opening a PR; CI runs the read-only variant
(`make ci`: `fmt --check` instead of `fmt`).

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
- A feature addition is not complete until spec/ documents it (new IDs) and
  the feature-status row in spec/README.md reflects reality (`done` only when
  implemented and tested), in the same change as the code.

## Pull requests

- Keep the change scoped to what the PR describes.
- Add or update the spec and its feature-status row alongside behavior
  changes, not in a follow-up.
- Add a test that fails without your change and passes with it.
- Run `make ci-local` before opening the PR.
