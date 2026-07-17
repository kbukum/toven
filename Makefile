PACKAGE_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# nextest profile (see .config/nextest.toml). Local runs use `default`
# (fail-fast, no retries); CI overrides this to `ci` (retries + slow-timeout for
# the real-subprocess integration tests) by exporting NEXTEST_PROFILE=ci.
NEXTEST_PROFILE ?= default

.PHONY: check fmt fmt-check lint test test-nextest test-doc structure doc deny coverage smoke smoke-repo benchmark release-dry-run release-artifacts act-ci act-supply-chain act-release-readiness

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
	@echo "==> Checking declare-only aggregators (lib.rs / mod.rs)..."
	@command -v ast-grep >/dev/null 2>&1 || { echo "structure: ast-grep not found — install with 'brew install ast-grep' or 'cargo install ast-grep --locked'"; exit 1; }
	@ast-grep scan

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

deny:
	cargo deny check advisories bans licenses sources

coverage:
	cargo llvm-cov --workspace --fail-under-lines 85 --fail-under-functions 80

# In-tree app smoke: drive the freshly-built app binaries over the committed
# fixtures via the `apps/*/tests` integration tests — `toven-rs` runs a full
# PLAN+APPLY, the umbrella `toven` a read-only PLAN cut, and `toven-go` the real
# driver handshake (`federation_smoke`). The same tests run under `make test`.
# Offline.
smoke:
	cargo nextest run --profile $(NEXTEST_PROFILE) -p toven -p toven-rs -p toven-go-app -E 'binary(/smoke$$/)'

# Binary smoke over an arbitrary real repository: drive the umbrella `toven`
# binary through `modules` + a PLAN cut (read-only, no APPLY). The repo must
# carry its own toven.toml. Example: make smoke-repo REPO=./rskit TASK=test
smoke-repo:
	./scripts/smoke-repo.sh "$(REPO)" "$(TASK)"

# Release-readiness benchmark: compare Toven orchestration against the native
# commands it runs, using the installed `toven` binary. Performance claims
# require this evidence. Example: make benchmark CASE=bench/cases/rskit.sh
benchmark:
	./scripts/benchmark.sh "$(CASE)"

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
