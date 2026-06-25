# Toven engineering

## Principles

1. Discover before deciding; reuse rskit and existing project helpers where they fit.
2. Toven and rskit are pre-stable, so prefer clean redesigns over compatibility detours.
3. Cascade-complete model changes through schema, normalization, planner, executor, output, tests, and docs.
4. User-owned argv is sacred: Toven validates and expands selectors, but does not infer hidden flags or silently rewrite commands.
5. Libraries do not print; CLI/reporting layers own user-facing output — and reserve `stdout` for the machine-readable stream (route human progress and diagnostics to `stderr`), rejecting flags on verbs that do not consume them.
6. Performance claims require benchmark evidence.
7. Place every injected contract in `toven-ports`, keep its concrete adapter in the consuming crate, and give it a `toven-testkit` double.

## Module placement and layering

Dependencies flow downward only, never upward:

| Layer | Crate(s) | Owns |
|-------|----------|------|
| L0 | `toven-model` | pure vocabulary + graph/topo/wave algorithms (the dependency root) |
| L1 | `toven-ports` | the port traits + the shared surface behind them |
| L2 | `toven-engine`, adapter crates (`toven-rust`/`toven-go`/`toven-command`) | orchestration + concrete adapters over the ports |
| L3 | `toven-cli` | CLI taxonomy, argv dispatch, stdio/Event projections |
| L4 | `apps/*` | thin wiring binaries |

- **Ports live in `toven-ports`.** Any seam the engine injects as `&dyn` (VCS, toolchain probe, source digest, cache lookup, …) or any contract an adapter implements (Provider/ConfiguredAdapter, ReleaseTarget, Reporter, RawOutputSink) is a port trait in `toven-ports`, as a declare-only responsibility folder per port (`vcs/`, `toolchain/`, `source/`, `cache/`, …). A lower layer that needs higher-layer behavior defines the contract here and the implementation is injected from above.
- **Concrete adapters live in the consuming crate, never beside the trait.** rskit-backed IO adapters (e.g. `ProcessToolchainProber`, `FsSourceDigest`, `NullCache`) live in `toven-engine`; ecosystem adapters live in `toven-<eco>`. The trait knows only `toven-model` + `rskit` + std/ports value types — no engine type leaks upward into a port.
- **Every port has exactly one shared double in `toven-testkit`** (`doubles/<port>.rs`, re-exported through the declare-only `doubles/mod.rs`). No port double is left stranded inline in a crate's `tests/`.
- **No test-only escape hatches on production public surfaces.** A recover-the-inner accessor used only by tests (`into_sink`, `into_inner`, …) is `#[cfg(test)]`-gated or removed; shared doubles expose recording accessors (cloneable shared state) so tests assert without recovering the owned value.

## Local validation

| Command | Purpose |
|---------|---------|
| `make check` | Canonical full gate: fmt, clippy, tests, docs, deny, structure, release build. |
| `make fmt` | Format code. |
| `make lint` | Clippy with denied warnings. |
| `make test` | Workspace cargo tests. |
| `make coverage` | Workspace coverage gate. |
| `make structure` | `mod.rs` declare-only guard across `crates/*`. |
| `make smoke` | End-to-end smoke: drives the `toven-rs` app over a fixture repo (modules + plan + build). |

The smoke harness runs offline against a committed fixture repo; the benchmark harness returns later in the workspace redesign.

Prefer validating changed modules/areas unless a broader gate is clearly necessary.

## Testing standards

- Use reusable fixtures and declarative case files instead of embedding large config strings in tests.
- Tests should be deterministic and avoid real network access.
- Runtime paths should surface typed errors instead of panics.

## Documentation policy

- Stable project documentation belongs in `docs/`.
- `tmp/` is only for active plans, handoff notes, or temporary research that is still being worked.
- Completed phase-history documents should be removed or summarized into stable docs instead of accumulating in `tmp/`.

## Release policy

The remaining release work must prove Toven works as an external tool:

1. Remove publish-blocking local path dependency assumptions.
2. Rehearse installation through the intended release path.
3. Use the installed `toven` binary for rskit adoption and benchmarks.
4. Produce checksums, release metadata, and signed/provenance-ready artifacts.
