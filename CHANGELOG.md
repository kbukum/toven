# Changelog

All notable changes to Toven are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until the first alpha release, entries accumulate under **Unreleased** and are maintained by release automation.

## [Unreleased]

### Added

- On-disk content cache backend (`FsContentCache` in `toven-engine`): a synchronous, content-addressed presence cache built on rskit-fs atomic writes that implements both injected cache ports — the read-only `CacheStore` queried by PLAN and the write-only `CacheWriter` driven by APPLY — so cache verdicts and successful-run records persist across invocations without bridging an async runtime into the pure planner.
- Task-level `shared_inputs` for broad cache invalidation and the initial installed-binary benchmark harness scaffold.
- APPLY execution over the planned unit graph, including fail-closed dependency gating, fail-fast cancellation, persistent readiness/teardown lifecycle, live persistent raw output routing, safe explicit command environment policy, and successful-run cache recording.
- User-facing `toven generate` workflow with safe stdout/write modes, deterministic TOML rendering, and Rust adapter config contributions.
- Project/profile/scope adapter configuration, Rust multi-manifest discovery, adapter-owned default Rust tasks, and explicit cross-scope dependency overlays.
- Developer workflow inspection commands, watch mode, persistent task readiness, JSONL run events, and cache stats/clean.
- Task execution, local successful-run cache records, cache-hit skipping, `toven explain`, and opt-in cached passthrough args via `cache_args = true`.
- Git-baseline affected-module planning with reverse-dependent closure, root-file fail-closed behavior, and `toven affected` / `plan --affected` CLI surfaces.
- Strict `toven.toml` loading, normalized workspace/profile/task config, and filesystem preset resolution backed by rskit config, validation, and filesystem utilities.

### Changed

- Reuse rskit foundations instead of hand-rolled standard-library code: content hashing for cache keys and source digests now goes through the new `rskit_util::hash` helper (replacing direct `blake3` use), the source-tree digest walk uses `rskit-fs` `sync_io::tree::walk_tree` (replacing a hand-rolled recursive `std::fs::read_dir`), and the default APPLY environment reads `PATH` via `rskit_util::env::get`. No behavior change.
