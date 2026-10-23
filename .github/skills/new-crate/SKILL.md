---
name: new-crate
description: "toven: Add a Rust crate in the correct layer with workspace wiring, lints, docs, and shared tests."
user-invocable: true
---

# New crate

Add a crate only when no existing crate owns the capability. Read [architecture](../../../docs/architecture.md#workspace-layers) for the layer map and the [placement rules](../../../docs/engineering.md#placement-and-layering) first.

## Steps

1. **Pick the owner.** Check the existing crates and [concern ownership](../../../docs/concern-owners.md). If rskit should own it, use the [rskit-reuse skill](../rskit-reuse/SKILL.md) instead.
2. **Pick the layer.** Dependencies point downward only. Capability goes in `crates/*`; `apps/*` only wire.
3. **Place ports.** A new port trait goes in `toven-ports`, its production adapter in the consuming crate, and one shared double in `toven-testkit/src/doubles/`.
4. **Create it.**

   ```bash
   cargo new --lib crates/toven-<name>
   ```

   The `crates/*` glob adds it to the workspace. Copy the `[package]` workspace inheritance and `[lints] workspace = true` from a sibling crate's `Cargo.toml`. Add `//!` crate docs. Keep `lib.rs` declare-only.

5. **Write tests first** with `toven-testkit` fixtures and doubles. See [tests and proof](../../../docs/engineering.md#tests-and-proof).
6. **Document it.** Add the crate to the [architecture](../../../docs/architecture.md) layer table and any affected user docs.
7. **Validate.**

   ```bash
   cargo clippy -p toven-<name> --all-targets --all-features -- -D warnings
   cargo nextest run -p toven-<name> --all-features
   make fmt-check
   make structure
   make check   # a new crate changes the workspace
   ```

## Acceptance

- [ ] The crate sits in the right layer and only depends downward.
- [ ] Ports, adapters, and doubles follow the placement rules.
- [ ] It inherits workspace lints and has crate docs.
- [ ] Tests exist and pass; `make check` passes.
- [ ] Architecture and user docs list it.
- [ ] The [Definition of Done](../../../docs/engineering.md#definition-of-done) holds.

Edits only. Commit, push, or create branches only when asked.
