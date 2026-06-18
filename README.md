# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml)
[![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml)
[![Release Readiness](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml)

Toven is a fast, argv-first development and CI task planner for multi-module
repositories. It discovers workspace modules, orders work by dependency graph,
and renders reviewable command batches before execution.

## Status

**Pre-alpha.** The current implementation includes strict configuration loading,
filesystem preset resolution, Rust workspace discovery, dependency-aware
batching, affected-module planning, command execution, cache-backed skipping,
affected/cache explanation, watch-mode reruns, persistent task readiness,
developer workflow inspection commands, adapter-owned default tasks, Rust
multi-manifest discovery, explicit cross-scope dependency overlays, and the
`toven generate` config adoption workflow. Additional discovery adapters will be
added in follow-up phases.

Toven is not published to crates.io yet. Until the first alpha release, install
from source after cloning the repository.

## Design

- **Language-agnostic engine** — scheduling and planning stay separate from
  language-specific discovery.
- **Explicit argv rendering** — generated commands are argument vectors by
  default; shell execution must be opted into intentionally.
- **Preset catalog** — reusable task definitions are TOML data, not hard-coded
  command branches.
- **Repository-shaped fixtures** — smoke coverage runs against temporary copies
  of curated repositories, with ad-hoc entrypoints for larger local repos.

## Local development

```bash
git submodule update --init --recursive
make check
make coverage
```

The workspace is mid-redesign into a hexagonal `crates/*` stack. The dependency
root, [`toven-model`](crates/toven-model), provides the shared vocabulary
(identity, dependency graph, plan, and event types) plus the pure graph
algorithms the engine and ports build on. The CLI apps, binary smoke harness, and
benchmark rehearsals return as the later redesign steps land.

Stable project documentation lives in [`docs/`](docs/). Start with
[`Installation`](docs/installation.md), [`Getting started`](docs/getting-started.md),
and the split [`Command reference`](docs/commands/README.md). Active plans and
handoff notes live in [`tmp/`](tmp/).

## Configuration preview

Toven loads strict TOML from `toven.toml`. Unknown fields are rejected early,
project roots are resolved relative to the config file, and command templates are
validated before planning.

Use `toven generate` to create an initial reviewable config. By default it
prints TOML to stdout; `--write` creates `root/toven.toml`, and `--overwrite` is
required before replacing an existing config. Rust generation emits
profile-level Cargo manifest discovery plus explicit standard Rust task argv;
pass repeated `--manifest path/to/Cargo.toml` values for repositories with
multiple independent manifests.

Very small hand-written Rust configs can still rely on adapter-provided
fallback Rust tasks:

```toml
[project]
name = "demo"
root = "."

[profiles.main]
adapter = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{module.package}"]
resource_group = "cargo:{project.root}"
```

Repositories with multiple Cargo manifests configure those as Rust adapter
discovery options. Scopes are optional named overrides when a subset needs
different discovery, execution, or task behavior.

```toml
[project]
name = "rskit"
root = "."
base_ref = "origin/main"

[profiles.main]
adapter = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{module.package}"]
resource_group = "cargo:{project.root}"

[profiles.main.discovery]
manifests = ["core/Cargo.toml", "contrib/Cargo.toml"]

[profiles.main.tasks]
nextest = { argv = ["cargo", "nextest", "run", "--manifest-path", "{module.manifest}", "-p", "{module.package}", "{args}"], cache_args = true }

[[overlays]]
from = { scope = "app", module = "api" }
to = { scope = "lib", module = "shared" }
```

Overlays are only for relationships an adapter cannot infer safely. Native Rust
discovery infers local Cargo path dependencies across configured manifests.

Tasks can also reference preset files. Project presets are resolved before user
presets from `.toven/lang/<language>/presets/<name>.toml`; user presets use the
same layout under the current user's home directory.

Run a task directly with `toven <task>` or `toven run <task>` when the task name
matches a built-in subcommand. Successful executions write local cache records
under `.toven/cache/`, and later runs skip modules whose exact source,
dependency, task, toolchain, shared-input, and cache-format inputs still match.
Use `--force` to skip cache reads while writing fresh success records, or
`--no-cache` to disable reads and writes. Use `--output jsonl` to reserve stdout
for stable newline-delimited run events; subprocess stdout is redirected to
stderr in JSONL mode so event consumers can parse every stdout line as JSON.
JSONL includes plan metadata, plan units, cache decisions, unit lifecycle,
persistent readiness, and final run summaries.

Affected planning narrows a plan to modules changed since a git baseline plus
their reverse dependents:

```bash
cargo run -- plan --affected --base origin/main --merge-base
cargo run -- affected --base origin/main --merge-base
cargo run -- test --affected --base origin/main --merge-base
cargo run -- test --watch
cargo run -- run modules
cargo run -- explain fixture-core test --base origin/main --merge-base
cargo run -- modules
cargo run -- graph --format dot
cargo run -- cache stats
cargo run -- cache clean
```

Short aliases are available for frequently used inspection commands:
`toven list` / `toven ls` for modules, `toven deps` for graph,
`toven cache info` for cache stats, and `toven cache clear` for cache clean.

Set `project.base_ref` in `toven.toml` to provide a default baseline. Without
`--base` or `project.base_ref`, affected detection compares `HEAD` to `HEAD`
and only local staged, unstaged, and untracked changes are considered.

Passthrough args disable cache by default because arbitrary flags can change
command semantics. For task definitions where passthrough args are deterministic
and should be part of the task key, set `cache_args = true`:

```toml
[profiles.main.tasks]
test = { argv = ["cargo", "test", "--manifest-path", "{module.manifest}", "{module.args}", "{args}"], cache_args = true, shared_inputs = ["Cargo.lock", "rust-toolchain.toml"] }
```

`shared_inputs` are plain workspace-relative files or directories that
invalidate every module using the task. They intentionally do not support
templates, globs, `.` components, parent paths, or absolute paths; use explicit
canonical-looking paths such as `Cargo.lock` instead of `./Cargo.lock` for
workspace manifests, lockfiles, toolchain files, lint config, and CI-relevant
config.

Persistent tasks opt out of cache automatically and can declare when they are
ready. Readiness can be immediate after start, a bounded health command, or a
literal stdout/stderr matcher:

```toml
[profiles.main.tasks]
dev = { argv = ["cargo", "run", "-p", "server"], persistent = true, ready_output = "listening", ready_timeout_seconds = 30 }
```

Watch mode uses filesystem events, debounces rapid saves, ignores `.git/`,
`.toven/`, `target/`, and `node_modules/`, then maps changed paths directly to
affected modules and reverse dependents before rerunning work.

`make release-artifacts` stages the crates.io package and checksum manifest in
`dist/`. CI also generates a CycloneDX SBOM and checks Sigstore tooling without
publishing the crate; version-tag runs attach GitHub provenance attestations.

## Community

Contributions and issue reports are welcome. Please read:

- [Contributing Guide](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)
- [Governance](GOVERNANCE.md)
- [Maintainers](MAINTAINERS.md)

## Repository workflow

Changes use Conventional Commits and small pull requests. Start with the
community and policy files on `main`, then add implementation, CI, and real
fixture coverage through focused review branches.

## License

Toven is distributed under the terms of the [MIT License](LICENSE).
