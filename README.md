# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml) [![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml) [![Release Readiness](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml)

Toven is an argv-first development and CI task planner for multi-module repositories. It discovers workspace modules, orders work by dependency graph, plans only what changed, caches successful results, and renders reviewable command batches before running them.

**Status:** Pre-alpha, installed from source. See [what Toven does](docs/product.md).

## Quick start

```bash
git submodule update --init --recursive
cargo install --path apps/toven --locked --force

cd your-repo
toven init                    # onboarding wizard writes toven.toml
toven plan check              # see what would run
toven check                   # run it
```

Full walkthrough: [getting started](docs/getting-started.md).

## What you get

- **Your commands stay yours.** Task argv lives in `toven.toml`; Toven plans and schedules but never rewrites what runs.
- **Affected planning.** `toven <task> --base origin/main --merge-base` runs only modules changed since a baseline, plus their dependents.
- **Result caching.** Successful runs are cached; later runs skip modules whose source, dependencies, task, toolchain, and shared inputs still match.
- **Parallel waves.** Ready modules run together while dependency order holds.
- **Reviewable plans.** `toven plan`, `toven affected`, and `toven explain` show what will run and the exact argv, before anything executes.

## Configuration

Toven loads one strict `toven.toml`. A minimal Rust config:

```toml
[project]
name = "demo"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = ["Cargo.toml"]
```

`init` seeds starter tasks for each ecosystem (`build`, `check`, `test`, `lint`, `format`, `doc`, `run`). They work like npm scripts: `toven <name>` runs the matching entry, and you add, rename, or remove them freely. Edit or add one under `[ecosystems.<id>.tasks.<name>]`:

```toml
[ecosystems.rust.tasks.test]
argv = ["cargo", "test", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
cache_args = true
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
```

See [what Toven does](docs/product.md) for the full config surface, [architecture](docs/architecture.md) for how it flows, and the [command reference](docs/commands/README.md) for every flag.

## Common commands

```bash
toven test --nocapture                 # run with passthrough args
toven test --watch                     # rerun affected tests on every change
toven affected test --base origin/main --merge-base
toven explain test --module rust:core  # show the exact planned argv
toven modules                          # list discovered modules
toven graph --format dot               # dependency graph as Graphviz
toven cache stats                      # inspect the local cache
```

## Documentation

Start with [`docs/`](docs/README.md): [installation](docs/installation.md), [getting started](docs/getting-started.md), and the [command reference](docs/commands/README.md).

## Local development

```bash
git submodule update --init --recursive
make check      # canonical gate: fmt-check, lint, test, structure, doc, deny, release build
```

The workspace is a hexagonal `crates/*` + `apps/*` stack rooted at [`toven-model`](crates/toven-model). See [engineering](docs/engineering.md) for standards and validation commands.

## Community

- [Contributing Guide](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)
- [Governance](GOVERNANCE.md)
- [Maintainers](MAINTAINERS.md)

## License

Toven is distributed under the [MIT License](LICENSE).
