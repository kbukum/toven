# Installation

Toven is currently installed from a source checkout. The first binary release will be `v0.1.0-alpha.1`; the direct-download commands in this page become executable only after that hosted Release exists.

## Supported binary targets

The `v0.1.0-alpha.1` distribution contract supports:

| Platform | Rust target |
|---|---|
| Linux x86-64 with glibc | `x86_64-unknown-linux-gnu` |
| Linux ARM64 with glibc | `aarch64-unknown-linux-gnu` |
| macOS x86-64 | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86-64 | `x86_64-pc-windows-msvc` |

Each target will have one archive named `toven-v0.1.0-alpha.1-<target>.tar.gz`, except Windows, which uses `.zip`. The hosted Release will also contain `SHA256SUMS`, keyless Sigstore signature and certificate files, a CycloneDX SBOM, and GitHub build provenance.

## Install from source today

Requirements:

- Git
- the Rust toolchain pinned by `rust-toolchain.toml`
- the tools used by repository tasks, such as Cargo or Go

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

`--version` and help use stdout. Installation or usage failures use stderr and return a non-zero exit status.

## Direct download after the first release

Choose the archive matching the machine, download that archive together with `SHA256SUMS`, and verify the checksum before extracting or executing it. For example, the macOS Apple-silicon asset will be:

```text
https://github.com/kbukum/toven/releases/download/v0.1.0-alpha.1/toven-v0.1.0-alpha.1-aarch64-apple-darwin.tar.gz
```

Checksum verification is mandatory:

```bash
shasum --ignore-missing -a 256 -c SHA256SUMS
```

Linux can use `sha256sum --ignore-missing -c SHA256SUMS`. Windows can use `Get-FileHash -Algorithm SHA256` and compare the result with the matching `SHA256SUMS` entry.

The release workflow will also publish the exact `cosign verify-blob` command and certificate identity for keyless signature verification. Do not install an archive that fails checksum, signature, or provenance verification.

Extract the archive and place `toven` (`toven.exe` on Windows) in a directory on `PATH`, then verify:

```bash
toven --version
```

The expected version is:

```text
toven 0.1.0-alpha.1
```

## Run from a checkout

```bash
cargo run --quiet --locked -p toven -- --help
```

## Upgrade

Source installations are upgraded from the desired checkout:

```bash
git pull --ff-only
git submodule update --init --recursive
cargo install --path apps/toven --locked --force
```

Binary installations are upgraded only to an explicitly selected immutable version. Download and verify the new version as a new artifact; do not use an unpinned latest-release URL in CI.

Continue with [getting started](getting-started.md).
