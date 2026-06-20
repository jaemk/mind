# Common developer tasks. Run `make help` for the list.

.PHONY: help build fmt fmt-check clippy test check ci release clean

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
	@echo "  release    tag v$(VERSION) and push it (triggers the release workflow)"
	@echo "             override: make release TAG=v1.2.3  (or VERSION=1.2.3)"
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

clean:
	cargo clean
