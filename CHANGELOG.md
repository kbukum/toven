# Changelog

All notable changes to Toven are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until the first alpha release, entries accumulate under **Unreleased** and are maintained by release automation.

## [Unreleased]

### Added

- Task-level `shared_inputs` for broad cache invalidation and the initial installed-binary benchmark harness scaffold.
- APPLY execution over the planned unit graph, including fail-closed dependency gating, fail-fast cancellation, persistent readiness/teardown lifecycle, live persistent raw output routing, safe explicit command environment policy, and successful-run cache recording.
- User-facing `toven generate` workflow with safe stdout/write modes, deterministic TOML rendering, and Rust adapter config contributions.
- Project/profile/scope adapter configuration, Rust multi-manifest discovery, adapter-owned default Rust tasks, and explicit cross-scope dependency overlays.
- Developer workflow inspection commands, watch mode, persistent task readiness, JSONL run events, and cache stats/clean.
- Task execution, local successful-run cache records, cache-hit skipping, `toven explain`, and opt-in cached passthrough args via `cache_args = true`.
- Git-baseline affected-module planning with reverse-dependent closure, root-file fail-closed behavior, and `toven affected` / `plan --affected` CLI surfaces.
- Strict `toven.toml` loading, normalized workspace/profile/task config, and filesystem preset resolution backed by rskit config, validation, and filesystem utilities.
