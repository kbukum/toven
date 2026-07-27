PACKAGE_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# The native target triple of the machine running Make, used to build/package
# a smoke-tested release binary without requiring a cross-compilation target
# override (see `release-artifacts` below). Recursively expanded (`=`, not
# `:=`) so `rustc -vV` runs only when a recipe actually references it, not on
# every Make invocation.
HOST_TARGET = $(shell rustc -vV | sed -n 's/host: //p')

# nextest profile (see .config/nextest.toml). Local runs use `default`
# (fail-fast, no retries); CI overrides this to `ci` (retries + slow-timeout for
# the real-subprocess integration tests) by exporting NEXTEST_PROFILE=ci. The
# value is read by nextest itself, so it flows through the `toven`-driven test
# gate unchanged.
export NEXTEST_PROFILE ?= default

# Dogfood: the mapped task, coverage, affected, and release gates run through the
# freshly built `toven` binary. CI-strength flags (`-D warnings`, `--all-targets`,
# `--release`, `--no-deps`) are supplied at the gate as passthrough after `--`,
# spliced verbatim at each task's `{args}` — the emitted task table stays minimal
# and gate strength lives with the gate, not the config. Override to an installed
# binary for speed with `make TOVEN=toven check`.
TOVEN ?= cargo run --quiet --locked -p toven --

.PHONY: check fmt fmt-check lint test test-nextest test-doc structure doc docs-serve docs-build deny coverage affected smoke smoke-repo benchmark golden bless verify-release-platform-filter release-dry-run release-plan release-artifacts release-checksums release-sbom-binary act-ci act-supply-chain act-release-readiness

# Canonical local/CI gate for the virtual workspace.
check: fmt-check lint test structure doc deny verify-release-platform-filter release-dry-run

# rustfmt is intentionally native: `make check` gates the whole workspace in a
# single fast rustfmt pass; the per-module `format`/`format-check` tasks remain
# available through `toven`.
fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Toven-driven clippy. The passthrough carries the CI-strength target/feature
# selection and the `-- -D warnings` deny level that the emitted `lint` task argv
# deliberately does not encode.
lint:
	$(TOVEN) lint -- --all-targets --all-features -- -D warnings

# Tests run via the Toven `test` task (nextest, fast, globally parallel). nextest
# does not execute doctests, so they run separately and natively under `test-doc`.
test: test-nextest test-doc

test-nextest:
	$(TOVEN) test -- --all-targets --all-features

test-doc:
	cargo test --workspace --all-features --doc

structure:
	@echo "==> Checking declare-only aggregators (lib.rs / mod.rs)..."
	@command -v ast-grep >/dev/null 2>&1 || { echo "structure: ast-grep not found — install with 'brew install ast-grep' or 'cargo install ast-grep --locked'"; exit 1; }
	@ast-grep scan

# Toven-driven rustdoc. RUSTDOCFLAGS supplies the deny-warnings gate and the
# passthrough supplies `--all-features` (gate rustdoc across every feature); the
# `doc` task already documents only the local crates (`--no-deps` is the
# baked-in default), so it is not repeated here.
doc:
	RUSTDOCFLAGS="-D warnings" $(TOVEN) doc -- --all-features

docs-serve:
	@command -v mdbook >/dev/null 2>&1 || { echo "docs-serve: mdbook not found — install with 'cargo install mdbook --locked'"; exit 1; }
	@command -v mdbook-mermaid >/dev/null 2>&1 || { echo "docs-serve: mdbook-mermaid not found — install with 'cargo install mdbook-mermaid --locked'"; exit 1; }
	mdbook serve docs --open

docs-build:
	@command -v mdbook >/dev/null 2>&1 || { echo "docs-build: mdbook not found — install with 'cargo install mdbook --locked'"; exit 1; }
	@command -v mdbook-mermaid >/dev/null 2>&1 || { echo "docs-build: mdbook-mermaid not found — install with 'cargo install mdbook-mermaid --locked'"; exit 1; }
	mdbook build docs

# Scenario-driven golden matrix: every `scenario.yaml` under
# `apps/toven/tests/golden/` is one reported case (zero per-case code; see
# docs/testing.md). The matrix also runs inside the canonical gate — `make
# check` → `test` → nextest `--all-targets` picks up the `golden` harness — so
# this target is the focused inner loop.
golden:
	cargo test --locked -p toven --test golden

# Regenerate goldens from live output (RSKIT_BLESS=1), then run the matrix in
# check mode to prove the regenerated tree is clean and deterministic.
bless:
	RSKIT_BLESS=1 cargo test --locked -p toven --test golden
	$(MAKE) golden

# Prove the release fixture wrapper fails closed when its filter matches no
# scenarios. Match the diagnostic as well as the nonzero exit so an unrelated
# cargo failure cannot satisfy the regression.
verify-release-platform-filter:
	@output="$$(TOVEN_RELEASE_SCENARIO_FILTER='no-such-release-scenario' ./scripts/verify-release-platform.sh 2>&1)"; status=$$?; \
	if [ "$$status" -eq 0 ] || ! printf '%s\n' "$$output" | grep -F "matched no release scenarios" >/dev/null; then \
		printf '%s\n' "$$output"; \
		echo "verify-release-platform-filter: expected a fail-closed zero-match result" >&2; \
		exit 1; \
	fi

deny:
	cargo deny check advisories bans licenses sources

# Toven owns coverage aggregation and gates the emitted profiles against the
# `[ecosystems.rust.coverage]` thresholds (line 80 / function 80 in toven.toml).
coverage:
	$(TOVEN) coverage

# Affected-only planning: exercise Toven's change-based module selection against
# the configured base ref without running anything.
affected:
	$(TOVEN) affected test

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
# nothing to publish yet. Validate workspace metadata and a Toven-driven release
# build instead.
release-dry-run:
	cargo metadata --format-version 1 --no-deps >/dev/null
	$(TOVEN) build -- --release --all-features

# Mutation-free release preview: the version cascade, readiness preflight, SBOM,
# and dependency graphs Toven would produce. Read-only, safe to run anywhere.
release-plan:
	$(TOVEN) release plan
	$(TOVEN) release status
	$(TOVEN) release readiness
	$(TOVEN) release sbom --out-dir target/toven/release/sbom
	$(TOVEN) release depgraphs --out-dir target/toven/release/depgraphs

# Real per-target release binary: build the release profile for the native
# host target and package it into the fixed-name dist/ archive that
# toven.toml's `release.host.assets` declares. The manually dispatched
# .github/workflows/release.yml runs the same packaging script for every
# matrix target (building via `cross` for the cross-compiled aarch64-linux
# target); the default (native) invocation is also the per-PR packaging
# smoke check.
release-artifacts:
	rm -rf dist
	mkdir -p dist
	cargo build --locked --release -p toven --target $(HOST_TARGET)
	./scripts/package-release-binary.sh "$(HOST_TARGET)" "target/$(HOST_TARGET)/release/$(if $(findstring windows,$(HOST_TARGET)),toven.exe,toven)"

# Combine every per-target archive already staged under dist/ (downloaded
# from the build matrix) into the immutable SHA256SUMS the hosted Release
# publishes and signs. Fails closed when no archives are staged: with
# nullglob an empty match would leave the checksum tool reading stdin and
# hang CI. Uses shasum when present, sha256sum otherwise (same fallback as
# scripts/verify-release-binary.sh); both emit the same SHA256SUMS format.
release-checksums:
	cd dist && bash -c 'shopt -s nullglob; archives=(toven-*.tar.gz toven-*.zip); if (( $${#archives[@]} == 0 )); then echo "release-checksums: no toven-* archives staged under dist/" >&2; exit 1; fi; if command -v shasum >/dev/null; then shasum -a 256 "$${archives[@]}" > SHA256SUMS; elif command -v sha256sum >/dev/null; then sha256sum "$${archives[@]}" > SHA256SUMS; else echo "release-checksums: neither shasum nor sha256sum is available" >&2; exit 1; fi'

# Copy the umbrella app's CycloneDX SBOM (already produced by `release-plan`'s
# `toven release sbom`) to the fixed dist/ path release.host.assets declares.
release-sbom-binary:
	mkdir -p dist
	cp target/toven/release/sbom/toven.cdx.json dist/toven-sbom.cdx.json

act-ci:
	act pull_request -W .github/workflows/ci.yml

act-supply-chain:
	act pull_request -W .github/workflows/supply-chain.yml

act-release-readiness:
	act pull_request -W .github/workflows/release-readiness.yml
