# Engineering

Standards for contributing to Toven.

## Principles

1. Discover before deciding; reuse rskit and existing project helpers where they fit.
2. Prefer clean redesigns over compatibility detours — Toven and rskit are pre-stable.
3. Cascade model changes through schema, normalization, planner, executor, output, tests, and docs together.
4. User-owned argv is sacred: Toven validates and expands selectors but never infers hidden flags or rewrites commands.
5. Libraries return typed data and typed errors. Only the CLI/reporting layer prints, and it reserves stdout for the machine-readable stream.
6. Performance claims require benchmark evidence (`make benchmark`).
7. Place every injected contract in `toven-ports`, keep its concrete adapter in the consuming crate, and give it a `toven-testkit` double.

## Module placement and layering

Dependencies flow downward only:

| Layer | Crate(s) | Owns |
|-------|----------|------|
| L0 | `toven-model` | pure vocabulary + graph/topo/wave algorithms (the dependency root) |
| L1 | `toven-ports` | the port traits + the shared surface behind them |
| L2 | `toven-engine`, adapter crates | orchestration + concrete adapters over the ports |
| L3 | `toven-cli` | CLI taxonomy, argv dispatch, stdio/Event projections |
| L4 | `apps/*` | thin wiring binaries |

- **Ports live in `toven-ports`.** Any seam the engine injects as `&dyn`, or any contract an adapter implements, is a port trait here, as a declare-only folder per port (`vcs/`, `toolchain/`, `source/`, `cache/`, …).
- **Concrete adapters live in the consuming crate.** rskit-backed IO adapters (`ProcessToolchainProber`, `FsSourceDigest`, `NullCache`) live in `toven-engine`; ecosystem adapters live in `toven-<eco>`. A port trait references only `toven-model` + rskit + std/ports value types.
- **Every port has one shared double in `toven-testkit`** (`doubles/<port>.rs`), re-exported through the declare-only `doubles/mod.rs`.
- **No test-only escape hatches on production surfaces.** A recover-the-inner accessor used only by tests is `#[cfg(test)]`-gated or removed; shared doubles expose recording accessors instead.

See [architecture](architecture.md) for the runtime flow through these layers.

## Local validation

| Command | Purpose |
|---------|---------|
| `make check` | Canonical full gate: fmt, clippy, tests, docs, deny, structure, release build. |
| `make fmt` | Format code. |
| `make lint` | Clippy with denied warnings. |
| `make test` | Workspace tests via nextest, plus doctests. Requires `cargo-nextest`. |
| `make coverage` | Workspace coverage gate. |
| `make structure` | `mod.rs` declare-only guard across `crates/*`. |
| `make smoke` | Run the in-tree app smokes over committed fixtures. |
| `make smoke-repo REPO=<path> [TASK=<task>]` | Drive the `toven` app over a real repo (read-only PLAN cut). |
| `make benchmark CASE=<case-file>` | Compare Toven against the native commands it runs. See [benchmarking](benchmarking.md). |

The shipping apps carry end-to-end smokes that drive the built binaries under `make test`/CI: `apps/toven-rs/tests/smoke.rs` (full PLAN+APPLY), `apps/toven/tests/smoke.rs` (read-only PLAN cut), `apps/toven-go/tests/federation_smoke.rs` (driver handshake).

Prefer validating changed modules unless a broader gate is clearly necessary.

## Testing

- Use reusable fixtures and declarative case files instead of embedding large config strings.
- Keep tests deterministic and free of real network access.
- Runtime paths surface typed errors, never panics.
- Regression-test every fix.

## Documentation

- Stable project documentation belongs in `docs/`; `tmp/` holds only active plans and handoff notes.
- Markdown is not hard-wrapped: one line per paragraph, preserving code blocks, mermaid, tables, and lists.

## Release policy

1. Remove publish-blocking local path dependency assumptions.
2. Rehearse installation through the intended release path.
3. Use the installed `toven` binary for adoption and benchmarks.
4. Produce checksums, release metadata, and signed/provenance-ready artifacts.
