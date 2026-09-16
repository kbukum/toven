# Getting started

This guide onboards a repository, shows what Toven discovered, and runs one task end to end.

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

`init` detects supported ecosystems, walks through a short wizard, previews the result, and writes `toven.toml`. Use `--non-interactive` when you want the default answers with no prompts.

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
Module             Workspace
command:repo       command
rust:toven-core    rust
rust:toven-cli     rust
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

Pass tool arguments unchanged after `--`:

```bash
toven test -- --nocapture
toven test -- --ignored
```

The explicit `--` sends every following flag to the underlying task instead of Toven. See [running tasks](commands/run.md).

## Preview or run only changed work

```bash
toven affected test --base origin/main --merge-base
toven plan test --base origin/main --merge-base
toven test --base origin/main --merge-base
```

`affected` and `plan` are read-only previews. `toven test --base origin/main --merge-base` runs that same changed-selection cut. When Toven cannot assign a change to one module, it expands to the full safe scope and reports why.

## Next steps

- [Core concepts](product.md)
- [Configuration guide](config/README.md)
- [Command reference](commands/README.md)
- [Release workflow](commands/release.md)
