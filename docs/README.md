# Toven documentation

Toven plans and runs development, CI, coverage, and release work across multi-module repositories. It discovers Rust workspaces and Go modules, builds their dependency graph, selects affected work, and executes the exact command arguments declared by the repository.

## Choose a path

| Goal | Start here |
|---|---|
| Install Toven and run a first task | [Getting started](getting-started.md) |
| Understand the product before adopting it | [Core concepts](product.md) |
| Configure a Rust or Go repository | [Configuration guide](config/README.md) |
| Find a command or flag | [Command reference](commands/README.md) |
| Plan and publish releases | [Release workflow](commands/release.md) |
| Integrate Toven into CI | [Self-hosting and CI](self-hosting.md) |
| Understand the implementation | [Architecture](architecture.md) |
| Contribute to Toven | [Engineering guide](engineering.md) |
| Add end-to-end test coverage | [Testing](testing.md) |

## Five-minute path

1. [Install Toven](installation.md).
2. Run `toven init` in a Git repository.
3. Review the generated `toven.toml`.
4. Run `toven modules`, `toven graph`, and `toven plan check`.
5. Run `toven check`.

```bash
toven init
toven modules
toven graph
toven plan check
toven check
```

Human progress and diagnostics are written to stderr. Read-only tables and machine-readable JSONL are written to stdout. See [output streams](commands/README.md#output-streams).

## Documentation map

- **Learn:** [installation](installation.md), [getting started](getting-started.md), [core concepts](product.md), [worked scenarios](scenarios.md)
- **Configure:** [configuration guide](config/README.md), [release configuration](config/release.md)
- **Operate:** [commands](commands/README.md), [release workflow](commands/release.md), [coverage](commands/coverage.md), [cache](commands/cache.md)
- **Contribute:** [architecture](architecture.md), [engineering](engineering.md), [concern ownership](concern-owners.md), [benchmarking](benchmarking.md)

## View locally

Build or serve the same Markdown as a searchable mdBook:

```bash
cargo install mdbook mdbook-mermaid --locked
make docs-build
make docs-serve
```

`make docs-serve` opens the site, renders Mermaid diagrams, and reloads it when a documentation file changes.
