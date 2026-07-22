# Installation

Toven is currently installed from a source checkout. The planned distribution is a checksum-verified binary downloaded from a GitHub Release.

## Requirements

- Git
- The Rust toolchain pinned by `rust-toolchain.toml`
- The tools used by repository tasks, such as Cargo or Go

## Install from source

```bash
git clone --recurse-submodules https://github.com/kbukum/toven.git
cd toven
cargo install --path apps/toven --locked --force
```

Verify the installation:

```bash
toven --version
toven --help
```

Expected stdout:

```text
toven <version>
```

Help is written to stdout. Installation or usage failures are written to stderr and return a non-zero exit status.

## Run from a checkout

Use the workspace binary without installing it:

```bash
cargo run --quiet --locked -p toven -- --help
```

## Upgrade

Rebuild from the desired checkout:

```bash
git pull --ff-only
git submodule update --init --recursive
cargo install --path apps/toven --locked --force
```

Continue with [getting started](getting-started.md).
