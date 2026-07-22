# Self-hosting and CI

Toven uses its own planner for mapped development and release previews. The Makefile remains the stable local and CI entry point.

## Binary selection

Until `v0.1.0-alpha.1` is published, Make targets run the freshly built workspace binary:

```makefile
TOVEN ?= cargo run --quiet --locked -p toven --
```

After the binary release is available, an installed and verified binary can be selected explicitly:

```bash
make TOVEN=toven check
```

CI must pin a Toven version and checksum. It must not use an unversioned latest-release URL.

## Canonical gate

```bash
make check
```

The gate includes formatting, linting, nextest, doctests, structure checks, rustdoc, dependency policy, and release build readiness.

| Gate | Execution |
|---|---|
| Lint, nextest, rustdoc, release build | Toven task |
| rustfmt workspace check | Native Cargo |
| Rust doctests | Native Cargo |
| Dependency policy | cargo-deny |
| Declare-only structure | ast-grep |

Additional entry points are:

```bash
make affected
make coverage
make release-plan
make smoke
```

`release-plan` runs release plan, readiness, SBOM, and dependency graph previews. It does not approve or perform a release.

## Release approval pipeline

A protected release workflow must:

1. Check out the exact source commit with full tag history and submodules.
2. Install a pinned Toven binary and verify its SHA-256 checksum, keyless Sigstore signature, certificate identity, and provenance.
3. Run plan, status, readiness, SBOM, dependency graph, and publication rehearsal commands.
4. Preserve machine-readable stdout and generated evidence as review artifacts.
5. Require a protected-environment approval tied to the exact commit and preview.
6. Recheck the branch, clean tree, selected version, and absence of conflicting immutable results.
7. Publish with least-privilege permissions.
8. Download and verify every registry artifact, tag, binary, checksum, signature, SBOM, provenance record, and hosted asset.

Human release tables and JSONL use stdout. Warnings and mutation progress use stderr. Automation should parse only stdout when `--output jsonl` is selected and retain stderr as diagnostics.

```bash
toven release publish --dry-run --output jsonl > release-preview.jsonl
```

## Toven's first release

The first release is `v0.1.0-alpha.1`. It is a hosted binary release, not a crates.io publication. Every workspace crate remains `publish = false`.

The release matrix is:

| Archive target | Format |
|---|---|
| `x86_64-unknown-linux-gnu` | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` |
| `x86_64-apple-darwin` | `.tar.gz` |
| `aarch64-apple-darwin` | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` |

Every archive contains one directly runnable binary. The hosted Release contains `SHA256SUMS`, keyless Sigstore/cosign signature and certificate files, a CycloneDX SBOM, and GitHub build provenance. Release verification downloads each archive and checks that `toven --version` reports `0.1.0-alpha.1`.

The current release-readiness workflow still builds a source archive rather than this matrix. The binary build, signing, publication, and download verification become executable in the bootstrap release step; they are not current capabilities.

## Immutability and recovery

Release tags, registry versions, hosted Releases, and approved assets are immutable. CI must fail if any intended output already exists with different content. A partially completed release is not repaired by moving tags, deleting registry versions, editing release notes, or clobbering assets. Correct the source or workflow, select a forward-fix version, regenerate the preview, and obtain approval again.

The current GitHub host adapter reconciles an existing Release in place. Protected release CI must not rely on that behavior; immutable host enforcement remains required before the first publication.

## GitHub Action direction

The planned `toven-action` is a thin installer and argv forwarder. It downloads a selected Toven release, verifies integrity, optionally caches the binary, and forwards argv unchanged. Release policy remains in `toven.toml` and Toven itself.

Pin the action to an immutable commit SHA and the binary to a version and checksum.

## Local workflow reproduction

When `act` is installed:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

These commands reproduce workflow structure locally but do not replace GitHub-hosted identity, signing, provenance, or release verification.
