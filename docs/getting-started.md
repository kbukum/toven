# Getting started

This guide configures a repository, previews the work Toven discovered, and runs one task.

## Prerequisites

- Toven installed and available on `PATH`
- A Git repository containing a Rust workspace, Go modules, or both
- The language tools required by the repository

See [installation](installation.md) before continuing.

## Initialize the repository

From the repository root:

```bash
toven init
```

`init` detects supported ecosystems, asks configuration questions, previews the result, and writes `toven.toml`.

Typical stderr:

```text
detected: rust
written: toven.toml
```

Preview without writing:

```bash
toven init --print
```

The generated TOML is written to stdout. Diagnostics remain on stderr, so redirecting stdout is safe:

```bash
toven init --print > toven.toml
```

See [`toven init`](commands/init.md) for all options.

## Review the configuration

A minimal Rust configuration identifies the repository and enables workspace discovery:

```toml
[project]
name = "example"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = "auto"
```

Generated task tables remain repository-owned. Toven executes the configured argv without silently adding flags. See the [configuration guide](config/README.md).

## Inspect discovery

```bash
toven modules
toven graph
toven tasks
```

Example stdout from `toven modules`:

```text
Module       Workspace
rust:core    rust
rust:cli     rust
```

Read-only projections use stdout. Warnings and final errors use stderr.

## Preview a task

```bash
toven plan check
toven explain check
```

`plan` reports the number of execution units and dependency waves. `explain` includes the exact argv for each planned unit.

Example stderr from the human plan reporter:

```text
plan: 2 units in 2 waves
```

See [inspection commands](commands/inspect.md).

## Run the task

```bash
toven check
```

Toven runs ready modules concurrently while preserving dependency order. Human progress, child-process output, and the run summary use stderr. A successful run exits with status `0`.

Pass tool arguments unchanged after Toven's option prefix:

```bash
toven test --nocapture
toven test -- --dry-run
```

The explicit `--` sends a flag that would otherwise be interpreted by Toven to the underlying task. See [running tasks](commands/run.md).

## Plan only changed work

```bash
toven plan test --base origin/main --merge-base
toven affected test --base origin/main --merge-base
```

Toven selects changed modules and the dependents that may be affected. A repository-level change that cannot be assigned to one module activates the complete scope and reports why.

## Next steps

- [Core concepts](product.md)
- [Configuration guide](config/README.md)
- [Command reference](commands/README.md)
- [Release workflow](commands/release.md)
