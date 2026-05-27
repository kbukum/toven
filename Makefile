.PHONY: check fmt fmt-check lint test doc deny dist-plan coverage act-ci act-supply-chain

check: fmt-check lint test doc deny dist-plan

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

act-ci:
	act pull_request -W .github/workflows/ci.yml

act-supply-chain:
	act pull_request -W .github/workflows/supply-chain.yml
