# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml) [![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml) [![Release Readiness](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml)

Plan and run development and CI tasks across multi-module repositories without rewriting the commands your repository already owns.

Toven is an argv-first task planner. It discovers workspace modules, orders work by dependency graph, selects affected work, caches successful results, and shows the exact command batches it will run before anything executes.

**Status:** Alpha. Signed binaries are published on the [Releases page](https://github.com/kbukum/toven/releases). You can also build from source. For the full product overview, see [core concepts](docs/product.md).

## Quick start

Install the latest signed binary on Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh | sh
```

Or use Homebrew (`brew tap kbukum/tap && brew install toven`), Scoop on Windows (`scoop install toven` after adding the bucket), or [build from source](docs/installation.md). Then, in your repository:

```bash
toven init                    # onboarding wizard writes toven.toml
toven plan check              # see what would run
toven check                   # run it
```

For a fuller walkthrough, see [getting started](docs/getting-started.md).

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
manifests = "auto"
```

`init` seeds starter tasks for each ecosystem (`build`, `check`, `test`, `lint`, `format`, `doc`, `run`). They work like npm scripts: `toven <name>` runs the matching entry, and you add, rename, or remove them freely. Edit or add one under `[ecosystems.<id>.tasks.<name>]`:

```toml
[ecosystems.rust.tasks.test]
argv = ["cargo", "test", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
cache_args = true
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
```

See [core concepts](docs/product.md) for the full config surface, [architecture](docs/architecture.md) for how it fits together, and the [command reference](docs/commands/README.md) for every flag.

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

Start with the [documentation home](docs/README.md), then jump to [installation](docs/installation.md), [getting started](docs/getting-started.md), [configuration](docs/config/README.md), [commands](docs/commands/README.md), or [release workflows](docs/commands/release.md). Every page renders directly on GitHub.

Run `make docs-serve` to open the same documentation as a searchable local mdBook with sidebar navigation and live reload.

## Local development

```bash
git submodule update --init --recursive
make check      # canonical gate: fmt-check, lint, test, structure, doc, deny, release build
```

The workspace is a hexagonal `crates/*` + `apps/*` stack: pure model and semver crates at the bottom, ports and focused mechanisms above them, planning and release engines in the middle, ecosystem adapters beside them, and thin app binaries on top. See [engineering](docs/engineering.md) for the full layout, standards, and validation commands.

## Community

- [Contributing Guide](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)
- [Governance](GOVERNANCE.md)
- [Maintainers](MAINTAINERS.md)

## License

Toven is distributed under the [MIT License](LICENSE).
