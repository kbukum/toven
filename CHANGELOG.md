# Changelog

All notable changes to Toven will be documented here by release automation.

## Unreleased

- Added task-level `shared_inputs` for broad cache invalidation and the initial
  installed-binary benchmark harness scaffold.
- Added the user-facing `toven generate` workflow with safe stdout/write modes,
  deterministic TOML rendering, and Rust adapter config contributions.
- Added project/profile/scope adapter configuration, Rust multi-manifest
  discovery, adapter-owned default Rust tasks, and explicit cross-scope
  dependency overlays.
- Added developer workflow inspection commands, watch mode, persistent task
  readiness, JSONL run events, and cache stats/clean.
- Added task execution, local successful-run cache records, cache-hit skipping,
  `toven explain`, and opt-in cached passthrough args via
  `cache_args = true`.
- Added git-baseline affected-module planning with reverse-dependent closure,
  root-file fail-closed behavior, and `toven affected`/`plan --affected` CLI
  surfaces.
- Added strict `toven.toml` loading, normalized workspace/profile/task config,
  and filesystem preset resolution backed by rskit config, validation, and
  filesystem utilities.
