# `toven-ports` test fixtures

Reusable input files for the crate's tests, kept out of the test bodies so the
data is readable on its own and shared across cases.

Layout (grouped by the surface under test, then by case):

- `config/ecosystems/<id>/` — valid `[ecosystems.<id>]` adapter configs, one
  folder per ecosystem so new ecosystems slot in without touching existing ones.
  - `rust/adapter.toml` — a flattened Rust adapter config (common knobs +
    adapter-specific `manifests`).
- `config/invalid/` — malformed configs that must be rejected (e.g. unknown keys).

Load fixtures with `include_str!` (relative to the consuming source file) so a
missing or renamed fixture is a compile error rather than a runtime surprise.
