# Installation

Toven is currently pre-alpha and is not published to crates.io yet. Until the
first alpha release, install it from a local checkout.

## Requirements

- Rust toolchain compatible with the repository `rust-toolchain.toml`
- Git, because affected planning compares working-tree changes against git
  baselines
- The build tools required by the repository commands you want Toven to run
  (for example Cargo for Rust workspaces)

## Install from source

Clone the repository, initialize submodules, and install the CLI binary:

```bash
git submodule update --init --recursive
cargo install --path . --locked --force
```

Confirm that the installed binary is the one on your `PATH`:

```bash
which toven
toven --version
toven --help
```

Use the installed `toven` binary for adoption checks and benchmarks. Avoid
mixing installed-binary evidence with `cargo run` output when evaluating release
readiness.

## Build locally without installing

For local development inside the Toven checkout:

```bash
make check
cargo run -- --help
```

`cargo run` is useful while developing Toven itself. User-facing docs and
benchmark evidence should use the installed `toven` binary.

## Next step

After installation, follow the [getting started guide](getting-started.md) to
generate a starter `toven.toml` and run your first task.
