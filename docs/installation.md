# Installation

This page shows the install paths this repository actually supports: released binaries, a source checkout, and generated package-manager channels when they are published.

Toven is distributed as signed, checksum-verified binaries on the [Releases page](https://github.com/kbukum/toven/releases). Every binary release is pinned by an immutable version tag, never by an unpinned latest-release URL, especially in CI.

## Quick install

On Linux or macOS, install the latest release into `~/.toven/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh | sh
```

On Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.ps1 | iex
```

Both scripts resolve the newest published tag, including prereleases, then download and verify that tag's exact assets. The archive is never trusted before its checksum verifies, and the Sigstore signature on `SHA256SUMS` is checked first when `cosign` is present. Pass a directory or pin a version explicitly:

```bash
curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh | sh -s -- --version v0.1.0-alpha.2 --dir /usr/local/bin
```

In CI, always pin the version and pin the script URL itself to a release tag (for example `.../kbukum/toven/v0.1.0-alpha.2/scripts/install.sh`) so no unpinned latest-release URL enters an automated pipeline.

## Package-manager channels

This repository includes packaging templates for Homebrew and Scoop under `packaging/`, and the release flow is set up to publish rendered manifests to `kbukum/homebrew-tap` and `kbukum/scoop-bucket`. Use these channels only when those distribution repositories have been published for the version you want. If you need the always-available path, use the release installer above or build from source.

When the Homebrew tap is published:

```bash
brew tap kbukum/tap
brew install toven
```

The one-shot equivalent is `brew install kbukum/tap/toven`. Upgrade with `brew upgrade toven`.

When the Scoop bucket is published:

```powershell
scoop bucket add toven https://github.com/kbukum/scoop-bucket
scoop install toven
```

## Supported binary targets

Each hosted release provides one fixed-name archive per target — `toven-<target>.tar.gz`, except Windows, which uses `.zip`:

| Platform | Rust target |
|---|---|
| Linux x86-64 with glibc | `x86_64-unknown-linux-gnu` |
| Linux ARM64 with glibc | `aarch64-unknown-linux-gnu` |
| macOS x86-64 | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86-64 | `x86_64-pc-windows-msvc` |

Archive names never embed the version — it lives in the release tag. The hosted Release also contains `SHA256SUMS`, its keyless Sigstore bundle (`SHA256SUMS.bundle`, carrying the signature, certificate, and verification material), a CycloneDX SBOM, and a GitHub build provenance attestation.

## Install from source

Requirements:

- Git
- the Rust toolchain pinned by `rust-toolchain.toml`
- the tools used by repository tasks, such as Cargo or Go

There is no `cargo install toven` from crates.io. Every workspace package is `publish = false`, so source installs use the checked-out `apps/toven` binary crate directly.

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

## Direct download

Choose a release version to pin, then download the archive matching the machine together with `SHA256SUMS` and verify the checksum before extracting or executing it. Set `TOVEN_VERSION` to the release tag you are pinning:

```bash
TOVEN_VERSION=v0.1.0-alpha.2
base="https://github.com/kbukum/toven/releases/download/${TOVEN_VERSION}"
```

For example, the macOS Apple-silicon asset is `${base}/toven-aarch64-apple-darwin.tar.gz`.

Checksum verification is mandatory:

```bash
shasum --ignore-missing -a 256 -c SHA256SUMS
```

Linux can use `sha256sum --ignore-missing -c SHA256SUMS`. Windows can use `Get-FileHash -Algorithm SHA256` and compare the result with the matching `SHA256SUMS` entry.

`SHA256SUMS` itself is keyless Sigstore/cosign-signed. Verify it before trusting the checksums it contains:

```bash
cosign verify-blob \
  --bundle SHA256SUMS.bundle \
  --certificate-identity-regexp 'https://github.com/kbukum/toven/.github/workflows/release.yml@.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
```

Do not install an archive that fails checksum, signature, or provenance verification.

Extract the archive and place `toven` (`toven.exe` on Windows) in a directory on `PATH`, then confirm the version you installed:

```bash
toven --version
```

It prints `toven <version>`, matching the release tag you pinned.

## Run without installing

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
