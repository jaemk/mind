# Common developer tasks. Run `make help` for the list.

.PHONY: help build build-release fmt fmt-check clippy test audit check ci ci-local release clean docs docs-build

# Package version from Cargo.toml, used to derive the release tag.
VERSION := $(shell grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
TAG := v$(VERSION)

help:
	@echo "targets:"
	@echo "  build      cargo build"
	@echo "  build-release  release binary with an embedded SBOM (cargo auditable)"
	@echo "  fmt        cargo fmt"
	@echo "  fmt-check  cargo fmt --check"
	@echo "  clippy     cargo clippy (all targets + features, warnings as errors)"
	@echo "  test       cargo test (all features)"
	@echo "  audit      cargo audit (RustSec advisories; skipped if not installed)"
	@echo "  check      local gate: fmt (fix) + clippy + test + audit"
	@echo "  ci         CI gate: fmt-check + clippy + test + audit"
	@echo "  ci-local   like ci but formats in place (fmt) instead of fmt-check"
	@echo "  release    tag v$(VERSION) and push it (triggers the release workflow)"
	@echo "             override: make release TAG=v1.2.3  (or VERSION=1.2.3)"
	@echo "  docs       build the docs site and serve it locally with live reload"
	@echo "  docs-build build the docs site to docs/book"
	@echo "  clean      cargo clean"

build:
	cargo build

# The same build the release workflow ships (.github/workflows/release.yml):
# `cargo auditable` embeds a dependency SBOM in the binary, so `cargo audit bin
# <path>` can scan an already-built artifact without its source or lockfile.
# Keep this in step with the workflow's build step.
build-release:
	@command -v cargo-auditable >/dev/null || { echo "error: cargo-auditable not found; install with 'cargo install cargo-auditable --locked'"; exit 1; }
	cargo auditable build --release --locked --bin mind

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

# Scan the locked dependency tree against the RustSec advisory database.
#
# Skipped with a note, not failed, when cargo-audit is absent: this runs inside
# `ci`, and .github/workflows/ci.yml's `check` jobs do not install the tool.
# They do not need to, because the dedicated `audit` job there already scans
# every push with rustsec/audit-check; this target exists so the same scan is
# exercised LOCALLY, before a push, rather than being first seen in CI.
# A hard failure here would therefore break CI to re-check what CI already does.
#
# `cargo audit` exits non-zero on a vulnerability but not on an informational
# advisory (`unmaintained`, `unsound`, `notice`), which matches the CI job's
# behavior: those are reported as warnings and are frequently in a transitive
# dependency with no fixed release to move to.
audit:
	@command -v cargo-audit >/dev/null 2>&1 \
		|| { echo "note: skipping audit; cargo-audit not installed (cargo install cargo-audit --locked)"; exit 0; }; \
	cargo audit

# Local developer gate: format in place, then lint, test, and scan advisories.
check: fmt clippy test audit

# CI gate: the same lints and tests, but verify formatting (fail if unformatted)
# rather than rewriting files. CI runs this; see .github/workflows/ci.yml.
ci: fmt-check clippy test audit

# Local pre-commit gate: identical to `ci` but formats in place (cargo fmt)
# instead of just checking, so a single command both fixes formatting and runs
# the full lint + test gate. Same set as `check`.
ci-local: fmt clippy test audit

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
