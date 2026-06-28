# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml) [![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml) [![Release Readiness](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml)

Toven is a fast, argv-first development and CI task planner for multi-module repositories. It discovers workspace modules, orders work by dependency graph, and renders reviewable command batches before execution.

## Status

**Pre-alpha.** The hexagonal `crates/*` + `apps/*` redesign is complete, with `toven-model`, `toven-ports`, `toven-engine`, `toven-cli`, `toven-rust`, `toven-go`, `toven-command`, `toven-testkit`, the `toven`, `toven-rs`, and `toven-go` apps, plus `examples/embed` in the workspace.

Toven is not published to crates.io yet. Install from source after cloning the repository.

## Design

- **Language-agnostic engine** — scheduling and planning stay separate from language-specific discovery.
- **Explicit argv rendering** — generated commands are argument vectors by default; shell execution must be opted into intentionally.
- **Adapter-owned defaults** — reusable task definitions come from adapter defaults as structured data, not hard-coded command branches.
- **Repository-shaped fixtures** — smoke coverage runs against temporary copies of curated repositories, with ad-hoc entrypoints for larger local repos.

## Local development

```bash
git submodule update --init --recursive
make check
make coverage
```

The workspace uses a hexagonal `crates/*` plus `apps/*` stack. The dependency root, [`toven-model`](crates/toven-model), provides the shared vocabulary and graph algorithms; ports, engine, adapters, CLI, apps, smoke harnesses, and benchmark rehearsals are wired end to end.

Stable project documentation lives in [`docs/`](docs/). Start with [`Installation`](docs/installation.md), [`Getting started`](docs/getting-started.md), and the split [`Command reference`](docs/commands/README.md).

## Configuration

Toven loads strict TOML from `toven.toml`. Unknown fields are rejected early, project roots are resolved relative to the config file, and command templates are validated before planning.

Use `toven generate` to create an initial reviewable config. By default it prints TOML to stdout; `--write` writes `<root>/toven.toml`. Re-runs are additive: they add missing `[ecosystems.<id>]` sections, preserve existing sections and `[project]`/`[toven]`, and `--force <id>` regenerates one ecosystem section. Rust generation emits ecosystem-level Cargo manifest discovery plus standard Rust task defaults.

Very small hand-written Rust configs can still rely on adapter-provided fallback Rust tasks:

```toml
[project]
name = "demo"
root = "."

[ecosystems.rust]
manifests = ["Cargo.toml"]
run_strategy = "leaf-to-top"
```

Repositories with multiple Cargo manifests list them under the Rust ecosystem's discovery options. Per-task argv overrides are optional named entries under `[ecosystems.<id>.tasks.<name>]` when a task needs different command or caching behavior.

```toml
[project]
name = "rskit"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = ["core/Cargo.toml", "contrib/Cargo.toml"]
run_strategy = "leaf-to-top"

[ecosystems.rust.tasks.nextest]
argv = ["cargo", "nextest", "run", "--manifest-path", "{module.manifest}", "-p", "{module.package}", "{args}"]
cache_args = true

[[overlays]]
from = { ecosystem = "go", module = "api" }
to = { ecosystem = "rust", module = "shared-types" }
```

Overlays are only for relationships an adapter cannot infer safely. Native Rust discovery infers local Cargo path dependencies across configured manifests.

Run a task directly with `toven <task>` or `toven run <task>` when the task name matches a built-in subcommand. Successful executions write cache records under the platform user-cache directory by default (`<app-cache>/toven/<workspace-hash>/v3`); set `TOVEN_CACHE_DIR` (an absolute path) or a workspace-relative `[toven.cache].dir` to relocate them. Later runs skip modules whose exact source, dependency, task, toolchain, shared-input, and cache-format inputs still match. Use `--output jsonl` to reserve stdout for stable newline-delimited run events; subprocess stdout is redirected to stderr in JSONL mode so event consumers can parse every stdout line as JSON. JSONL serializes the typed event stream: run start/finish (with the run summary), phase markers, the `plan-prepared` event (wave and unit counts), per-unit `cache-decided` verdicts, and unit lifecycle events (start, ready, finished).

Affected planning narrows a plan to modules changed since a git baseline plus their reverse dependents:

```bash
toven plan test --base origin/main --merge-base
toven affected test --base origin/main --merge-base
toven test --base origin/main --merge-base
toven explain rust:fixture-core test
toven modules
toven graph --format dot
toven cache stats
toven cache clean
```

Short aliases are available for frequently used inspection commands: `toven list` / `toven ls` for modules and `toven deps` for graph. Cache maintenance uses `toven cache stats`, `toven cache clean`, and `toven cache path`.

Set `project.base_ref` in `toven.toml` to provide a default baseline. Affected detection requires a baseline: with neither `--base` nor `project.base_ref`, it fails with a `no baseline reference` error rather than selecting any modules.

Passthrough args disable cache by default because arbitrary flags can change command semantics. For task definitions where passthrough args are deterministic and should be part of the task key, set `cache_args = true`:

```toml
[ecosystems.rust.tasks.test]
argv = ["cargo", "test", "--manifest-path", "{module.manifest}", "{module.args}", "{args}"]
cache_args = true
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
```

`shared_inputs` are plain workspace-relative files or directories that invalidate every module using the task. They intentionally do not support templates, globs, `.` components, parent paths, or absolute paths; use explicit canonical-looking paths such as `Cargo.lock` instead of `./Cargo.lock` for workspace manifests, lockfiles, toolchain files, lint config, and CI-relevant config.

Persistent tasks opt out of cache automatically and can declare when they are ready. Readiness can be immediate after start, a bounded health command, or a literal stdout/stderr matcher:

```toml
[ecosystems.rust.tasks.dev]
argv = ["cargo", "run", "-p", "server"]
persistent = true
ready_output = "listening"
ready_timeout_seconds = 30
```

`make release-artifacts` stages the crates.io package and checksum manifest in `dist/`. CI also generates a CycloneDX SBOM and checks Sigstore tooling without publishing the crate; version-tag runs attach GitHub provenance attestations.

## Community

Contributions and issue reports are welcome. Please read:

- [Contributing Guide](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)
- [Governance](GOVERNANCE.md)
- [Maintainers](MAINTAINERS.md)

## Repository workflow

Changes use Conventional Commits and small pull requests. Start with the community and policy files on `main`, then add implementation, CI, and real fixture coverage through focused review branches.

## License

Toven is distributed under the terms of the [MIT License](LICENSE).
