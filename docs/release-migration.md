# Release migration map: Toven

This is the gate-by-gate map from the raw commands Toven's workspace used before self-hosting to the `toven`-driven gate that replaces it. Each row lists the original gate, the Toven command (and the `toven.toml` task or `release` subcommand it drives), the expected output, and the raw command that remains runnable by hand for parity during the migration. The narrative overview is in [`self-hosting.md`](self-hosting.md).

`$(TOVEN)` is `cargo run --quiet --locked -p toven --` (override with an installed binary via `make TOVEN=toven <target>`).

## Quality gates

| Old gate | Toven command / task | Expected output | Retained raw command |
|---|---|---|---|
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | `make lint` → `$(TOVEN) lint -- --all-targets --all-features -- -D warnings` (`[ecosystems.rust.tasks.lint]`) | PLAN + per-module `clippy` run, non-zero exit on any warning | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `cargo nextest run --workspace --all-features` | `make test-nextest` → `$(TOVEN) test -- --all-targets --all-features` (`[ecosystems.rust.tasks.test]`) | PLAN + globally parallel nextest run summary | `cargo nextest run --workspace --all-targets --all-features` |
| `cargo test --workspace --all-features --doc` | `make test-doc` (intentionally native — nextest does not run doctests) | doctest run summary | `cargo test --workspace --all-features --doc` |
| `cargo doc --workspace --no-deps` (`RUSTDOCFLAGS=-D warnings`) | `make doc` → `RUSTDOCFLAGS="-D warnings" $(TOVEN) doc` (`[ecosystems.rust.tasks.doc]`, `--no-deps` baked in) | PLAN + rustdoc build, non-zero exit on any doc warning | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` |
| `cargo llvm-cov --workspace ...` | `make coverage` → `$(TOVEN) coverage` (`[ecosystems.rust.tasks.coverage]`) | coverage profile aggregation gated on `[ecosystems.rust.coverage]` thresholds | `cargo llvm-cov --workspace --lcov --output-path target/toven/coverage/rust.lcov` |
| `cargo fmt --all --check` | `make fmt-check` (intentionally native — one fast whole-workspace pass) | formatting diff, non-zero exit on drift | `cargo fmt --all --check` |
| `cargo deny check advisories bans licenses sources` | `make deny` (intentionally native — supply-chain policy) | cargo-deny report | `cargo deny check advisories bans licenses sources` |
| `ast-grep scan` (declare-only aggregator guard) | `make structure` (intentionally native) | structure guard report | `ast-grep scan` |
| — (change-based selection was manual) | `make affected` → `$(TOVEN) affected test` | affected-module table for the configured `base_ref`, no execution | `git diff --name-only origin/main...HEAD` |

## Release gates

Toven ships signed, provenance-attested build artifacts from a `v*` tag and never publishes its crates to crates.io (every crate is `publish = false`). There is no legacy crates.io publish path to retain — the raw column below is the underlying build/metadata command, not a competing release system.

| Old gate | Toven command / task | Expected output | Retained raw command |
|---|---|---|---|
| workspace metadata + release build | `make release-dry-run` → `cargo metadata --no-deps` + `$(TOVEN) build -- --release --all-features` | metadata sanity check + release-profile build | `cargo metadata --format-version 1 --no-deps` + `cargo build --workspace --release --all-features` |
| version-cascade preview | `make release-plan` → `$(TOVEN) release plan` | semver-cascade table (current → planned per module) | — (Toven-owned; no prior tool) |
| release preflight | `$(TOVEN) release readiness` (via `make release-plan`) | readiness table with go / no-go verdict (`clean-tree`) | `git status --porcelain` |
| SBOM generation | `$(TOVEN) release sbom --out-dir target/toven/release/sbom` (via `make release-plan`) | per-crate CycloneDX SBOMs under `target/toven/release/sbom` | `cargo cyclonedx --format json` |
| dependency graphs | `$(TOVEN) release depgraphs --out-dir target/toven/release/depgraphs` (via `make release-plan`) | DOT graph under `target/toven/release/depgraphs` | — (Toven-owned) |
| signed source artifact | `make release-artifacts` | `dist/toven-<version>-source.tar.gz` + `dist/SHA256SUMS` | `tar` + `shasum -a 256` |

## Parity notes

- Every `release plan`, `release readiness`, `release sbom`, and `release depgraphs` invocation is mutation-free: it writes only under `target/` and `dist/` and never mutates tracked files, tags, or remotes.
- `release readiness` fails `no-go` on a dirty working tree, so `make release-plan` is expected to fail locally when run against uncommitted changes — that is the clean-tree guardrail, not a regression.
- The retained raw commands stay available for the whole migration so any Toven result can be cross-checked against the tool it wraps.
