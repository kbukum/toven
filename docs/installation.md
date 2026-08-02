# Installation

Toven is distributed as signed, checksum-verified binaries on the [Releases page](https://github.com/kbukum/toven/releases), and can also be installed from a source checkout. Binary releases are pinned by an immutable version tag — never an unpinned latest-release URL, especially in CI.

## Quick install

On Linux or macOS, install the latest release into `~/.toven/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh | sh
```

On Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.ps1 | iex
```

Both resolve the newest published tag (including prereleases), then download and verify that tag's exact assets — the archive is never trusted before its checksum verifies, and the Sigstore signature on `SHA256SUMS` is checked first when `cosign` is present. Pass a directory or pin a version explicitly:

```bash
curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh \
  | sh -s -- --version v0.1.0-alpha.2 --dir /usr/local/bin
```

In CI, always pin the version and pin the script URL itself to a release tag (for example `.../kbukum/toven/v0.1.0-alpha.2/scripts/install.sh`) so no unpinned latest-release URL enters an automated pipeline.

## Homebrew

```bash
brew tap kbukum/tap
brew install toven
```

This taps `kbukum/homebrew-tap` and installs the signed release binary for your platform. As a one-shot equivalent, `brew install kbukum/tap/toven` also works. Upgrade with `brew upgrade toven`.

## Scoop (Windows)

```powershell
scoop bucket add toven https://github.com/kbukum/scoop-bucket
scoop install toven
```

## Supported binary targets

Each hosted release provides one fixed-name archive per target:

| Platform | Rust target |
|---|---|
| Linux x86-64 with glibc | `x86_64-unknown-linux-gnu` |
| Linux ARM64 with glibc | `aarch64-unknown-linux-gnu` |
| macOS x86-64 | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86-64 | `x86_64-pc-windows-msvc` |

Each target has one fixed-name archive, `toven-<target>.tar.gz`, except Windows, which uses `toven-<target>.zip`. Archive names never embed the release version — the version lives in the release tag instead. The hosted Release also contains `SHA256SUMS`, a keyless Sigstore signature and certificate for it (`SHA256SUMS.sig`, `SHA256SUMS.pem`), a CycloneDX SBOM, and a GitHub build provenance attestation.

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
  --certificate SHA256SUMS.pem \
  --signature SHA256SUMS.sig \
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
