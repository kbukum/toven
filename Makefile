PACKAGE_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# nextest profile (see .config/nextest.toml). Local runs use `default`
# (fail-fast, no retries); CI overrides this to `ci` (retries + slow-timeout for
# the real-subprocess integration tests) by exporting NEXTEST_PROFILE=ci.
NEXTEST_PROFILE ?= default

.PHONY: check fmt fmt-check lint test test-nextest test-doc structure doc deny coverage smoke release-dry-run release-artifacts act-ci act-supply-chain act-release-readiness

# Canonical local/CI gate for the virtual workspace.
check: fmt-check lint test structure doc deny release-dry-run

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Tests run via nextest (fast, globally parallel across every test binary).
# nextest does not execute doctests, so they run separately under `test-doc`.
test: test-nextest test-doc

test-nextest:
	cargo nextest run --profile $(NEXTEST_PROFILE) --workspace --all-targets --all-features

test-doc:
	cargo test --workspace --all-features --doc

structure:
	./scripts/check-structure.sh

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

deny:
	cargo deny check advisories bans licenses sources

coverage:
	cargo llvm-cov --workspace --fail-under-lines 85 --fail-under-functions 80

# Managed end-to-end smoke: drive the toven-rs binary over the single-rust
# fixture (modules + plan + build). Offline; no real network access.
smoke:
	./scripts/smoke.sh

# Every crate is currently an unpublished, path-dependent library, so there is
# nothing to publish yet. Validate workspace metadata and a release build instead.
release-dry-run:
	cargo metadata --format-version 1 --no-deps >/dev/null
	cargo build --workspace --release --all-features

# Ship a reproducible source tarball until publishable apps land.
release-artifacts:
	rm -rf dist
	mkdir -p dist
	tar --exclude './.git' --exclude '*/.git' --exclude './target' --exclude './dist' --exclude './tmp' -czf dist/toven-$(PACKAGE_VERSION)-source.tar.gz .
	( cd dist && shasum -a 256 * > SHA256SUMS )

act-ci:
	act pull_request -W .github/workflows/ci.yml

act-supply-chain:
	act pull_request -W .github/workflows/supply-chain.yml

act-release-readiness:
	act pull_request -W .github/workflows/release-readiness.yml
