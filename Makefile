# Common developer tasks. Run `make help` for the list.

.PHONY: help build fmt fmt-check clippy test check ci ci-local release clean docs docs-build

# Package version from Cargo.toml, used to derive the release tag.
VERSION := $(shell grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
TAG := v$(VERSION)

help:
	@echo "targets:"
	@echo "  build      cargo build"
	@echo "  fmt        cargo fmt"
	@echo "  fmt-check  cargo fmt --check"
	@echo "  clippy     cargo clippy (all targets + features, warnings as errors)"
	@echo "  test       cargo test (all features)"
	@echo "  check      local gate: fmt (fix) + clippy + test"
	@echo "  ci         CI gate: fmt-check + clippy + test"
	@echo "  ci-local   like ci but formats in place (fmt) instead of fmt-check"
	@echo "  release    tag v$(VERSION) and push it (triggers the release workflow)"
	@echo "             override: make release TAG=v1.2.3  (or VERSION=1.2.3)"
	@echo "  docs       build the docs site and serve it locally with live reload"
	@echo "  docs-build build the docs site to docs/book"
	@echo "  clean      cargo clean"

build:
	cargo build

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

# Local developer gate: format in place, then lint and test.
check: fmt clippy test

# CI gate: the same lints and tests, but verify formatting (fail if unformatted)
# rather than rewriting files. CI runs this; see .github/workflows/ci.yml.
ci: fmt-check clippy test

# Local pre-commit gate: identical to `ci` but formats in place (cargo fmt)
# instead of just checking, so a single command both fixes formatting and runs
# the full lint + test gate. Same set as `check`.
ci-local: fmt clippy test

# Tag the current commit and push it, which triggers .github/workflows/release.yml.
# Defaults to v<Cargo.toml version>; override with `make release TAG=v1.2.3` or
# `make release VERSION=1.2.3`. Requires a clean tree and an unused tag.
release:
	@test -z "$$(git status --porcelain)" || { echo "error: working tree is dirty; commit first"; exit 1; }
	@if git rev-parse -q --verify "refs/tags/$(TAG)" >/dev/null; then \
		echo "error: tag $(TAG) already exists"; exit 1; \
	fi
	git tag -a $(TAG) -m "release $(TAG)"
	git push origin $(TAG)

# Serve the mdBook docs (docs/) locally with live reload, opening a browser.
# Same tool the Pages workflow uses; install with `cargo install mdbook` (or grab
# a prebuilt binary from the mdBook releases).
docs:
	@command -v mdbook >/dev/null || { echo "error: mdbook not found; install with 'cargo install mdbook'"; exit 1; }
	mdbook serve docs --open

# Build the static site to docs/book (what CI deploys to Pages).
docs-build:
	@command -v mdbook >/dev/null || { echo "error: mdbook not found; install with 'cargo install mdbook'"; exit 1; }
	mdbook build docs

clean:
	cargo clean
