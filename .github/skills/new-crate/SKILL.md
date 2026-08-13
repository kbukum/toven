---
name: new-crate
description: >-
    Scaffold a new crate in Toven's hexagonal Cargo workspace the canonical way — place it in the
    right layer (model/ports/engine/adapter/cli), honor the binding port-placement rule, wire the
    workspace, inherit workspace lints (#![forbid(unsafe_code)], missing_docs), and add its shared
    double to toven-testkit. Use when adding a capability, port, adapter, or crate to Toven.
user-invocable: true
---

# Adding a crate to Toven

Toven is one Cargo workspace (`members = ["crates/*", "apps/*"]`, `exclude = ["rskit"]`) arranged as a hexagonal stack that depends **downward only**. Getting layer and port placement right up front avoids layering-by-convention violations that the compiler won't catch.

## Step 1 — Pick the layer (downward-only)

- **L0 `toven-model`** — pure vocabulary: identity, dependency graph, plan, event types, and graph algorithms. Depends on no other Toven crate (only rskit + third-party like `serde`).
- **L0.5 `toven-semver`** — pure semver toolkit: bump math and the release-tag codec/selection, wrapping `rskit_version::semver`. Reusable by any layer; depends on no other Toven crate.
- **L1 `toven-ports`** — hexagonal port traits (Provider/ConfiguredAdapter, ReleaseTarget, Reporter, RawOutputSink, VcsReader/VcsWriter, ToolchainProber, SourceDigest, CacheStore) and helpers (template, merge, config). Depends on `toven-model` + rskit.
- **L1.5 focused mechanism crates** — `toven-exec` (concrete subprocess runners behind the runner ports) and `toven-vcs` (the rskit-git-backed `VcsReader`/`VcsWriter` adapter + change foundation). Pure mechanism; policy stays engine-owned. Depend on `toven-ports` + rskit.
- **L2 capability/engine crates** — `toven-core` (PLAN foundation + strict config `Document` loader; the L2 floor), `toven-version` (the pure `plan_bumps` version-decision capability over `toven-semver`), `toven-release` and `toven-engine` (PLAN/APPLY coordination + concrete rskit-backed adapters for injected ports). `toven-{rust,go,command}` are the L2 ecosystem adapters implementing the ports; they never reach into engine or cli.
- **L3 `toven-cli`** — CLI taxonomy, argv-first dispatch, and the only layer that prints.
- **L4 `apps/*`** — thin wiring binaries (`apps/toven`, `apps/toven-rs`, `apps/toven-go`); each wires adapters into `toven-cli`. No new capability goes here — add it to the right `crates/*` layer instead.
- **`toven-testkit`** — dev-only (`publish = false`): fixtures API, port doubles, sample-repo/git scenario helpers.

A lower layer importing a higher one is a **blocker**. `make structure` guards `mod.rs` declare-only placement.

## Step 2 — Honor the binding port-placement rule

When the crate introduces or implements a port:

- The **port trait lives in `toven-ports`**; it references only `toven-model` + rskit + std/ports value types — **no engine type leaks upward**.
- Its **concrete adapter lives in the consuming crate** (engine or `toven-<eco>`), never beside the trait.
- Every port has **exactly one shared double in `toven-testkit`** (`doubles/<port>.rs`) — never a stranded inline double in a crate's `tests/`.

## Step 3 — Create and wire the crate

```bash
cargo new --lib crates/toven-<name>
```

- Add it to the workspace `members` (the `crates/*` glob usually covers it).
- Inherit workspace lints: `unsafe_code = "forbid"` and `missing_docs = "warn"` are set workspace-wide — add `///` docs to every public item and `//!` crate docs.
- `#[must_use]` on `with_*` builders; `#[non_exhaustive]` on public enums that may grow; no `unwrap()`/`expect()` in library code; use rskit `AppError`/`AppResult` preserving cause.
- **Libraries don't print** — only `toven-cli` produces user-facing output; library crates return typed data and typed errors.
- No test-only escape hatch on a production public surface (`into_inner`, `into_sink`, …): `#[cfg(test)]`-gate or remove it; shared doubles expose recording accessors instead.
- Organize by focused, concern-named files — never pile unrelated logic into one file; `lib.rs`/`mod.rs` stay declare-only (`make structure`).

## Step 4 — Validate

```bash
git submodule update --init --recursive
cargo clippy -p toven-<name> --all-targets --all-features -- -D warnings
cargo test   -p toven-<name> --all-features -q
make fmt-check
make structure
```

## Checklist

- [ ] Layer chosen (model/ports/engine/adapter/cli/testkit); dependencies downward-only
- [ ] Port trait (if any) in `toven-ports`; adapter in the consuming crate; one shared double in `toven-testkit`
- [ ] Added to workspace members; `#![forbid(unsafe_code)]` + `missing_docs` honored with `///` docs
- [ ] Public API typed/minimal; builders `#[must_use]`; growable enums `#[non_exhaustive]`
- [ ] Library returns typed data/errors — no printing outside `toven-cli`; no test-only public escape hatch
- [ ] `make structure` clean; clippy/test green for the crate

Per repo workflow, **create the branch and make edits only** — the maintainer commits and pushes.
