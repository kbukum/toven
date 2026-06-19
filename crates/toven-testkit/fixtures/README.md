# `toven-testkit` shared fixtures

The one shared fixture tree, read by tests across **all** Toven crates via the
`toven-testkit::fixtures` API (loaders resolve relative to this crate's
`CARGO_MANIFEST_DIR`, so a fixture added here is reachable from any consumer).

A crate adds a *local* `tests/fixtures` only for data that is genuinely
single-crate and never reused; anything a second step could want goes here.

## Layout

- `config/` — whole-`toven.toml` Document inputs (step 3).
  - `valid/` — well-formed Documents.
  - `invalid/` — inputs that must be rejected (unknown keys, bad strictness).
- `ecosystems/<id>/` — **ecosystem-specific**, isolated per id. Adding a new
  ecosystem never edits another ecosystem's files.
  - `rust/adapter/` — flattened `[ecosystems.rust]` adapter configs.
  - `rust/workspaces/` — standalone sample cargo workspaces for `cargo_metadata`
    discovery (step 4). Each is its own `[workspace]` root.
  - `go/` — placeholder; mirrors `rust/` when the Go adapter lands.
- `repos/` — full sample Toven-app repos the real CLI plans/applies against,
  materialized into temp dirs by `SampleRepo` (integration / e2e smoke).
  - `single-rust/` — one-ecosystem repo (plan/apply happy path); the seed repo.
  - `umbrella-multi-eco/` — multi-ecosystem umbrella (federation, steps 11/13).
  - `cross-repo/` — multi-repo `[[members]]` set (step 12).

## Loading

Load via the `toven_testkit::fixtures` API (`document`, `ecosystem`, `repo_path`)
or `SampleRepo::materialize`. A missing or renamed fixture surfaces as a clear
error, never a silent skip.
