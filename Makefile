# Common developer tasks. Run `make help` for the list.

.PHONY: help build fmt fmt-check clippy test check release clean

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
	@echo "  check      fmt-check + clippy + test"
	@echo "  release    tag v$(VERSION) and push it (triggers the release workflow)"
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

# The gates CI should enforce, runnable in one shot locally.
check: fmt-check clippy test

# Tag the current commit as v<Cargo.toml version> and push it, which triggers
# .github/workflows/release.yml. Requires a clean tree and an unused tag.
release:
	@test -z "$$(git status --porcelain)" || { echo "error: working tree is dirty; commit first"; exit 1; }
	@if git rev-parse -q --verify "refs/tags/$(TAG)" >/dev/null; then \
		echo "error: tag $(TAG) already exists"; exit 1; \
	fi
	git tag -a $(TAG) -m "release $(TAG)"
	git push origin $(TAG)

clean:
	cargo clean
