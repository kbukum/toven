CARGO_PACKAGE_DIRTY_FLAG ?= --allow-dirty

.PHONY: check fmt fmt-check lint test doc deny dist-plan coverage release-dry-run release-artifacts act-ci act-supply-chain act-release-readiness

check: fmt-check lint test doc deny dist-plan release-dry-run

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-targets --all-features

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

deny:
	cargo deny check advisories bans licenses sources

dist-plan:
	cargo metadata --format-version 1 --no-deps >/dev/null
	cargo build --release --all-features

coverage:
	cargo llvm-cov --lcov --ignore-filename-regex 'src/main.rs' --fail-under-lines 85 --fail-under-functions 80

release-dry-run:
	cargo package --locked $(CARGO_PACKAGE_DIRTY_FLAG) --list >/dev/null
	cargo publish --dry-run --locked $(CARGO_PACKAGE_DIRTY_FLAG)

release-artifacts:
	rm -rf dist
	mkdir -p dist
	cargo package --locked $(CARGO_PACKAGE_DIRTY_FLAG)
	cp target/package/toven-*.crate dist/
	( cd dist && shasum -a 256 * > SHA256SUMS )

act-ci:
	act pull_request -W .github/workflows/ci.yml

act-supply-chain:
	act pull_request -W .github/workflows/supply-chain.yml

act-release-readiness:
	act pull_request -W .github/workflows/release-readiness.yml
