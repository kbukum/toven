# Installation

Install the `toven` binary from a local checkout.

## Requirements

- A Rust toolchain matching `rust-toolchain.toml` (the workspace `rust-version` sets the minimum).
- Git — affected planning diffs your working tree against a git baseline.
- The tools your tasks invoke (for example Cargo for Rust workspaces).

## Install from source

```bash
git submodule update --init --recursive
cargo install --path apps/toven --locked --force
```

Confirm the binary on your `PATH`:

```bash
which toven
toven --version
toven --help
```

## Run from the checkout without installing

While developing Toven itself:

```bash
cargo run -p toven -- --help
```

Use the installed `toven` binary for adoption checks and benchmarks so evidence reflects a real install.

## Next step

Generate a config and run your first task with the [getting started guide](getting-started.md).
