CARGO_PACKAGE_DIRTY_FLAG ?= --allow-dirty
REQUIRE_PUBLISHABLE_PACKAGE ?= 0
HAS_PATH_DEPENDENCIES := $(shell grep -Eq 'path[[:space:]]*=' Cargo.toml && echo 1 || echo 0)
PACKAGE_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

.PHONY: check fmt fmt-check lint test smoke smoke-repo smoke-clone smoke-add-submodule smoke-add-case smoke-add-managed-submodule smoke-purge smoke-update doc deny dist-plan coverage release-dry-run release-artifacts act-ci act-supply-chain act-release-readiness

check: fmt-check lint test doc deny dist-plan release-dry-run

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	TOVEN_SMOKE_SKIP_MANAGED=1 cargo test --all-targets --all-features

smoke:
	./scripts/smoke.sh run

smoke-repo:
	./scripts/smoke.sh repo "$(REPO)" $(ARGS)

smoke-clone:
	./scripts/smoke.sh clone "$(URL)" "$(NAME)"

smoke-add-submodule:
	./scripts/smoke.sh add-submodule "$(URL)" "$(NAME)"

smoke-add-case:
	./scripts/smoke.sh add-case "$(NAME)" "$(REPO)" $(ARGS)

smoke-add-managed-submodule:
	./scripts/smoke.sh add-managed-submodule "$(URL)" "$(NAME)" $(ARGS)

smoke-purge:
	./scripts/smoke.sh purge "$(NAME)"

smoke-update:
	./scripts/smoke.sh update "$(NAME)"

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
	@set -e; \
	if [ "$(REQUIRE_PUBLISHABLE_PACKAGE)" = "1" ] || [ "$(HAS_PATH_DEPENDENCIES)" != "1" ]; then \
		cargo package --locked $(CARGO_PACKAGE_DIRTY_FLAG) --list >/dev/null; \
		cargo publish --dry-run --locked $(CARGO_PACKAGE_DIRTY_FLAG); \
	else \
		cargo package --locked $(CARGO_PACKAGE_DIRTY_FLAG) --no-verify --list >/dev/null; \
		echo "Skipping cargo publish --dry-run because Cargo.toml contains pre-release path dependencies."; \
		echo "Set REQUIRE_PUBLISHABLE_PACKAGE=1 once those dependencies are published."; \
	fi

release-artifacts:
	rm -rf dist
	mkdir -p dist
	@if [ "$(REQUIRE_PUBLISHABLE_PACKAGE)" = "1" ] || [ "$(HAS_PATH_DEPENDENCIES)" != "1" ]; then \
		cargo package --locked $(CARGO_PACKAGE_DIRTY_FLAG); \
		cp target/package/toven-*.crate dist/; \
	else \
		echo "Building pre-release source artifact because Cargo.toml contains path dependencies."; \
		tar --exclude './.git' --exclude '*/.git' --exclude './target' --exclude './dist' --exclude './tmp' -czf dist/toven-$(PACKAGE_VERSION)-source.tar.gz .; \
	fi
	( cd dist && shasum -a 256 * > SHA256SUMS )

act-ci:
	act pull_request -W .github/workflows/ci.yml

act-supply-chain:
	act pull_request -W .github/workflows/supply-chain.yml

act-release-readiness:
	act pull_request -W .github/workflows/release-readiness.yml
