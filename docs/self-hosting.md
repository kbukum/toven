# Self-hosting: Toven runs its own gates

Toven drives its own quality and release gates through the freshly built `toven` binary. The `Makefile` is a thin set of passthrough invocations, so the same argv-first planner that CI and downstream repositories use also gates Toven's own workspace. This document explains how the wiring works and why a few gates remain intentionally native.

## The `toven` entry point

Every mapped gate goes through one variable:

```makefile
TOVEN ?= cargo run --quiet --locked -p toven --
```

CI builds `toven` on the fly with `cargo run`, so a checkout needs no pre-installed binary. For faster local runs, point the variable at an installed binary:

```sh
make TOVEN=toven check
```

## Passthrough semantics

A gate invokes `toven <task> -- <underlying args>`. Toven splices whatever follows `--` verbatim at the task's `{args}` placeholder in `toven.toml`, so CI-strength flags live with the gate, not baked into the emitted task table. The emitted `lint`, `test`, and `build` tasks stay minimal; strength such as `-D warnings`, `--all-targets`, `--all-features`, and `--release` is supplied at the gate.

Tools that themselves consume `--` need a second separator. Clippy is the clearest example — the first `--` ends Toven's passthrough, the second is handed to `cargo clippy` so `-D warnings` reaches the lint driver rather than Toven:

```makefile
lint:
	$(TOVEN) lint -- --all-targets --all-features -- -D warnings
```

The `doc` task is different: `--no-deps` is the baked-in default of the emitted `doc` task (documenting dependencies is noise a user never wants from `toven doc`), so the gate adds no passthrough of its own and only supplies `RUSTDOCFLAGS="-D warnings"`.

## The `make check` gate set

`make check` preserves the complete gate set and exit behavior:

```makefile
check: fmt-check lint test structure doc deny release-dry-run
```

- `lint`, `test`, `doc`, and the `release-dry-run` build run through `toven`.
- `test` is `test-nextest` (the Toven `test` task, nextest, globally parallel) plus native doctests — nextest does not execute doctests, so they run separately under `test-doc`.
- `fmt-check`, `structure`, and `deny` are intentionally native (see below).

## Extra Toven-driven targets

- `make affected` runs `toven affected test` — change-based module selection against the configured `base_ref` without executing anything.
- `make coverage` runs `toven coverage`, gating the emitted profiles against the `[ecosystems.rust.coverage]` thresholds in `toven.toml`.
- `make release-plan` is the mutation-free release preview: `toven release plan`, `release readiness`, `release sbom`, and `release depgraphs`. It is read-only and safe to run anywhere; artifacts land under `target/toven/release/`.

## Intentionally native gates

A few gates deliberately stay on raw tooling because Toven does not replace them:

- **`cargo fmt --all` / `--all --check`** — `make check` gates the whole workspace in one fast rustfmt pass. The per-module `format` / `format-check` tasks remain available through `toven`.
- **Rust doctests (`cargo test --doc`)** — nextest does not run them, so `test-doc` invokes cargo directly.
- **`cargo deny check`** — dependency advisory, license, and source policy is a supply-chain concern Toven does not own.
- **`ast-grep scan` (`make structure`)** — the declare-only aggregator guard for `lib.rs` / `mod.rs`.

## CI wiring

- **`.github/workflows/ci.yml`** runs `make check` on the pinned toolchains, exercising the `toven`-driven mapped, structure, doc, and dependency gates.
- **`.github/workflows/release-readiness.yml`** runs `make release-dry-run` (a `cargo metadata` sanity check plus a `toven build --release`) and a dedicated mutation-free `release-plan` job that previews the version cascade, readiness verdict, SBOM, and dependency graphs. It also builds the signed source tarball, SBOM, and provenance a tagged release ships.

All actions are pinned by commit SHA. A release is a signed, provenance-attested build from a `v*` tag; Toven never publishes its crates to crates.io (every crate is `publish = false`).

The gate-by-gate mapping to raw commands is in [`release-migration.md`](release-migration.md).
