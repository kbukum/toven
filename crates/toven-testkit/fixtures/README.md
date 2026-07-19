# `toven-testkit` shared fixtures

The one shared fixture tree, read by tests across **all** Toven crates via the `toven-testkit::fixtures` API (loaders resolve relative to this crate's `CARGO_MANIFEST_DIR`, so a fixture added here is reachable from any consumer).

A crate adds a *local* `tests/fixtures` only for data that is genuinely single-crate and never reused; anything shared across crates goes here.

## Layout

- `config/` — whole-`toven.toml` Document inputs.
  - `valid/` — well-formed Documents.
  - `invalid/` — inputs that must be rejected (unknown keys, bad strictness).
- `ecosystems/<id>/` — **ecosystem-specific**, isolated per id. Adding a new ecosystem never edits another ecosystem's files.
  - `rust/adapter/` — flattened `[ecosystems.rust]` adapter configs.
  - `rust/workspaces/` — standalone sample cargo workspaces for `cargo_metadata` discovery. Each is its own `[workspace]` root.
  - `go/` — mirrors `rust/`; adapter configs and sample Go workspaces for `go mod edit` discovery.
- `repos/` — full sample Toven-app repos the real CLI plans/applies against, materialized into temp dirs by `SampleRepo` (integration / e2e smoke). Single-ecosystem repos are grouped by ecosystem so `toven` and each `toven-<eco>` binary map cleanly onto the repos they exercise.
  - `rust/` — Rust-only repos: `single/` (plan/apply happy path; the seed repo), `multi-module/` (intra-repo dependency graph), `multi-workspace/` (multiple cargo workspaces), `workspace-inherited/` (workspace-inherited config), `init-target/` (onboarding target, no `toven.toml`).
  - `go/` — Go-only repos: `single/`, `multi-module/` (`go.work`).
  - `command/` — command-ecosystem repos: `failing-task/` (non-zero exit path).
  - `cross-ecosystem/` — mixed-ecosystem repos in one tree: `umbrella/` (rust + go + command).
  - `federation/` — multi-repo `[[members]]` sets: `cross-repo/`.
  - `misc/` — edge-case repos: `no-ecosystem/` (no detectable ecosystem manifest; exercises `toven init` guidance path).

## Loading

Load via the `toven_testkit::fixtures` API (`document`, `ecosystem`, `repo_path`) or `SampleRepo::materialize`. A missing or renamed fixture surfaces as a clear error, never a silent skip.
