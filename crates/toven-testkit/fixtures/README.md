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
  - `js/` — adapter configs and sample npm, pnpm, Yarn, and Bun workspaces for js discovery.
- `coverage/` — coverage profiles the engine reads: `rust.lcov`, Go profiles (`go.out`, `go-nested.out`), and `js/` LCOV captured from real Vitest, Jest, c8, and `bun test` runs, each with package-relative paths.
- `repos/` — the **shape catalog**: full sample Toven-app repos ("worlds") the real CLI plans/applies against, materialized into temp dirs by `SampleRepo`. Each repo is a real, minimal, buildable tree named by ecosystem + topology, isolating exactly one shape.
  - `_profiles/` — the **shared task grammar**, one includable fragment per ecosystem (`rust-tasks.toml`, `go-tasks.toml`, `command-tasks.toml`). Every repo's `toven.toml` declares only `[project]` + its discovery shape and `include`s its profile — no repo restates the grammar. Config includes may not traverse above the config root, so `SampleRepo::materialize` copies `_profiles/` into the materialized repo root. (Federation *members* are the one exception: they cannot reach the umbrella root, so they carry a deliberate minimal grammar.)
  - `rust/` — `single/` (happy path; the seed repo), `workspace-linear/` (`app -> corelib -> util` chain), `workspace-diamond/` (`app -> {liba, libb} -> core`), `multi-workspace/` (two independent workspaces), `workspace-inherited/` (workspace-inherited config, no `toven.toml`), `publish-train/` (release: registry + per-module override), `umbrella-registry/` (release: umbrella-facade workspace `kit-suite -> kit-core -> kit-util` on the registry+umbrella baseline, with `toven.<mode>.toml` variants pinning each tag mode and the maintainer entrypoint), `onboarding/` (init target, no `toven.toml`).
  - `go/` — `single/`, `work-linear/` (`go.work`, `app -> core`), `versioned/` (`/v2` module path).
  - `command/` — toolchain-independent, deterministic: `single/` (echo-only), `failing/` (deliberately failing `check`), `multi-task/` (two modules + a task override).
  - `js/` — `bun-workspace/` (discovery), `tasks/` (the task shape `init` writes), `doctor/` (pinned npm, no install), `coverage/` (package-relative LCOV staged without a package manager), `onboarding-{npm,pnpm,yarn-berry,bun}/` (init targets, no `toven.toml`), `release-train/` (release: an npm workspace with a dependent cascade, a private site, and a `toven.manifest.toml` variant), `plainworks-shape/` (adoption: plainworks' Bun workspace and its `verify` gates as one set, with private apps and tooling excluded from release), and `smoke-{npm,bun}/` (real install/test/coverage loops run by the `toven-js` smokes).
  - `polyglot/` — `umbrella/` (rust + go + command under one `toven.toml`), `web-stack/` (adoption: runlab's Go service, two Bun roots, and a `proto` command module, with overlays and a `file:` tarball as a cache input).
  - `federation/` — multi-repo `[[members]]` sets: `cross-repo/`.
  - `edge/` — `no-ecosystem/` (no detectable manifest; `toven init` guidance path), `empty/` (nothing at all).
  - **Config variants**: where one source tree must exercise several situations, sibling `toven.<variant>.toml` files live beside the default `toven.toml` (e.g. `rust/workspace-linear/toven.unordered.toml`, `toven.json-report.toml`, `toven.custom-cache-dir.toml`), selected per run via `--config toven.<variant>.toml`.
- `scenarios/` — `scenario.yaml` documents for the scenario schema loader and engine (`toven_testkit::scenario`).
  - `valid/` — well-formed scenario definitions.
  - `invalid/` — definitions the loader must reject (unknown keys, unsafe step ids, traversing configs, unknown matcher tiers/toolchains), one directory per malformation.
  - `engine/` — runnable sessions (scenario + goldens) the engine tests drive with a deterministic fake binary: ordered cold→warm state, argv-verbatim proof, mismatch/exit/effect failure paths, toolchain skip.

## Loading

Load via the `toven_testkit::fixtures` API (`document`, `ecosystem`, `repo_path`, `scenario_path`) or `SampleRepo::materialize`. A missing or renamed fixture surfaces as a clear error, never a silent skip.
