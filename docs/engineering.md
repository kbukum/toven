# Engineering

Standards for contributing to Toven.

## Principles

1. Discover before deciding; reuse rskit and existing project helpers where they fit.
2. Prefer clean redesigns over compatibility detours — Toven and rskit are pre-stable.
3. Cascade model changes through schema, normalization, planner, executor, output, tests, and docs together.
4. Keep user-owned argv unchanged: Toven validates and expands selectors but never infers hidden flags or rewrites commands.
5. Libraries return typed data and typed errors. Only the CLI/reporting layer prints, and it reserves stdout for the machine-readable stream.
6. Performance claims require benchmark evidence (`make benchmark`).
7. Place every injected contract in `toven-ports`, keep its concrete adapter in the consuming crate, and give it a `toven-testkit` double.

## Module placement and layering

Dependencies flow downward only:

| Layer | Crate(s) | Owns |
|-------|----------|------|
| L0 | `toven-model` | pure vocabulary + graph/topo/wave algorithms (the dependency root) |
| L1 | `toven-ports` | the port traits + the shared surface behind them |
| L2 | `toven-engine`, adapter crates | coordination + concrete adapters over the ports |
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
| `make check` | Canonical full gate: fmt-check, lint, test, structure, doc, deny, release build. |
| `make fmt` | Format code. |
| `make fmt-check` | Check formatting without modifying files. |
| `make lint` | Clippy with denied warnings. |
| `make test` | Workspace tests via nextest, plus doctests. Requires `cargo-nextest`. |
| `make structure` | `mod.rs` declare-only guard across `crates/*`. Requires `ast-grep`. |
| `make doc` | Build docs with denied warnings. |
| `make deny` | Dependency/license/advisory audit via `cargo-deny`. |
| `make coverage` | Workspace coverage gate. Requires `cargo-llvm-cov`. |
| `make smoke` | Run the in-tree app smokes over committed fixtures. |
| `make smoke-repo REPO=<path> [TASK=<task>]` | Drive the `toven` app over a real repo (read-only PLAN cut). |
| `make benchmark CASE=<case-file>` | Compare Toven against the native commands it runs. See [benchmarking](benchmarking.md). |

The shipping apps carry end-to-end smokes that drive the built binaries under `make test`/CI: `apps/toven-rs/tests/` and `apps/toven/tests/` each split a PLAN-cut smoke (`smoke_plan.rs`) from a real-subprocess APPLY smoke (`smoke_apply.rs`), and `apps/toven-go/tests/federation_smoke.rs` drives the real `toven-go` driver handshake.

Prefer validating changed modules unless a broader gate is clearly necessary.

## Testing

- Use reusable fixtures and declarative case files instead of embedding large config strings.
- Keep tests deterministic and free of real network access.
- Runtime paths surface typed errors, never panics.
- Regression-test every fix.

## Documentation

- Stable project documentation belongs in `docs/`; `tmp/` holds only active plans and handoff notes.
- Write Markdown paragraphs as natural, continuous source lines. Do not hard-wrap prose to a column limit or insert source newlines for visual presentation; Markdown renderers handle viewport-aware wrapping. Keep intentional structure such as paragraph breaks, headings, lists, blockquotes, tables, mermaid diagrams, and fenced or indented code blocks.
- Apply the same rule to prose in `//!`/`///` rustdoc and `//` comments: do not introduce arbitrary column-based breaks. Preserve rustdoc formatting conventions for code examples, directives, lists, and tables. The `rustfmt` `max_width` limit is for code, not prose.

## Release policy

1. Remove publish-blocking local path dependency assumptions.
2. Test installation through the intended release path.
3. Use the installed `toven` binary for adoption and benchmarks.
4. Produce checksums, release metadata, and signed/provenance-ready artifacts.
