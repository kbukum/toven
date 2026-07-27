# Pass 00 — Structure and placement

Confirm every touched (or, in project mode, every existing) item lives in the right crate and layer, and that the hexagonal port rules hold. This is the first gate: misplaced code makes every later pass moot, so reject on failure here before going further.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* check the crates the diff touches plus their affected area. *Project mode:* sweep each crate's `Cargo.toml` dependency block and `src/` tree; the layering and port-placement rules below are rules for the whole workspace, not just a diff.

## The layering rule

Dependencies flow **downward only**, never upward:

| Layer | Crate(s) | Owns |
|-------|----------|------|
| L0 | `toven-model` | pure vocabulary + graph/topo/wave algorithms (the dependency root) |
| L1 | `toven-ports` | port traits + the shared surface behind them |
| L2 | `toven-engine`, adapters (`toven-rust`/`toven-go`/`toven-command`) | coordination + concrete adapters over the ports |
| L3 | `toven-cli` (and the `toven` library facade) | CLI taxonomy, argv dispatch, stdio/Event projections |
| L4 | `apps/*` | thin wiring binaries |

A lower layer importing a higher one is a **blocker**.

## Checks

- **No upward deps.** Inspect each touched crate's `Cargo.toml` and `use` statements. `toven-model` may depend only on rskit (`rskit-errors`, `rskit-validation`, …) + third-party (`serde`, `serde_json`). *Any* Toven dependency in `toven-model` is a blocker. Adapters (`toven-rust`/`go`/`command`) may depend on `toven-ports` + `toven-model` only — never on engine, cli, or apps.
- **No engine/cli type leaks into a port.** A `toven-ports` trait may reference only `toven-model` + rskit + std/ports value types. An engine or cli type in a port signature is a blocker — the contract belongs to the lower layer and the implementation is injected from above.
- **Port placement.** Every seam the engine injects as `&dyn` (VCS, toolchain probe, source digest, cache lookup, …) or any contract an adapter implements (Provider/ConfiguredAdapter, ReleaseTarget, Reporter, RawOutputSink) is a port trait in `toven-ports`, as a declare-only responsibility folder per port (`vcs/`, `toolchain/`, `source/`, `cache/`, …). The trait lives in `toven-ports`; its concrete adapter lives in the **consuming** crate (rskit-backed IO adapters like `ProcessToolchainProber`/`FsSourceDigest`/`NullCache` in `toven-engine`; ecosystem adapters in `toven-<eco>`), **never** beside the trait.
- **Exactly one shared double per port.** Each port has a single double in `toven-testkit` (`doubles/<port>.rs`, re-exported through the declare-only `doubles/mod.rs`). A port double stranded inline in some crate's `tests/` is a should-fix.
- **`lib.rs`/`mod.rs` are declare-only.** Declarations / re-exports only — no logic, no private items (crate-root `#![...]` attributes allowed). Gated by `make structure`; run it. Logic in an aggregator is a structure violation (blocker).
- **File homes.** New files sit in the crate that matches their layer. A new "shared" helper that is really a duplicated rskit concern does not belong anywhere here — hand it to pass `01` (rskit reuse).
- **Config ownership.** The single strict `Document` loader for `toven.toml` lives in `toven-engine`; the shared `[ecosystems.<id>]` vocabulary (`CommonEcosystemConfig`) lives in `toven-ports` and each adapter flattens it in its own `configure`. A second parse path or a loader outside the engine is a should-fix.

## Detection starters

These flag candidates, not verdicts — read each hit to judge intent.

```bash
# upward / cross-layer imports
rg 'use (toven_engine|toven_cli)' crates/toven-model/src crates/toven-ports/src crates/toven-rust/src
rg 'use (toven_engine|toven_cli|toven_rust)' crates/toven-ports/src
# what each crate actually depends on
for c in crates/*/Cargo.toml; do echo "== $c =="; rg '^toven-' "$c"; done
```

Then run `make structure` for the `mod.rs`/placement guard.
