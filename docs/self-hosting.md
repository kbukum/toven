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

| Archive target | Format | Archive name |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `.tar.gz` | `toven-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` | `toven-aarch64-unknown-linux-gnu.tar.gz` |
| `x86_64-apple-darwin` | `.tar.gz` | `toven-x86_64-apple-darwin.tar.gz` |
| `aarch64-apple-darwin` | `.tar.gz` | `toven-aarch64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` | `toven-x86_64-pc-windows-msvc.zip` |

Archive names are fixed and never embed the version: `toven.toml`'s `[ecosystems.rust.release.host]` `assets` list is a set of exact, non-templated project-relative paths (globbing and version placeholders are not implemented — see `crates/toven-ports/src/config/release/host.rs`), so the same static list must resolve on every release. The version lives in the release tag and Release title instead.

Every archive contains one directly runnable binary. The hosted Release also contains a combined `SHA256SUMS`, its keyless Sigstore/cosign signature and certificate (`SHA256SUMS.sig`, `SHA256SUMS.pem`), a CycloneDX SBOM (`toven-sbom.cdx.json`), and a separate GitHub build provenance attestation (not a listed asset; verify it with `gh attestation verify`).

`.github/workflows/release.yml` builds this matrix, assembles the fixed `dist/` file set, and runs `toven release publish` behind the protected `release` environment's required-reviewer approval. It is dispatched manually (`workflow_dispatch`) rather than triggered by a `v*` tag push, because `toven release publish` creates that tag itself; a tag-triggered run would race its own immutable-tag preflight. `scripts/package-release-binary.sh` packages one target's built binary into its fixed archive name; `scripts/verify-release-binary.sh` verifies a packaged or downloaded archive — for downloaded archives, first the keyless Sigstore signature on `SHA256SUMS`, then the archive's checksum — and, where the runner can execute the target, runs it. The cross-compiled `aarch64-unknown-linux-gnu` target builds through `cross` for a matching glibc/OpenSSL/libgit2, and is signature- and checksum-verified only — no hosted runner can execute a Linux ARM64 binary.

## Immutability and recovery

Release tags, registry versions, hosted Releases, and approved assets are immutable. CI must fail if any intended output already exists with different content. A partially completed release is not repaired by moving tags, deleting registry versions, editing release notes, or clobbering assets. Correct the source or workflow, select a forward-fix version, regenerate the preview, and obtain approval again.

The current GitHub host adapter is immutable create-or-verify: it creates the Release, or — when one already exists — reads it back and verifies it matches the intended Release exactly, hard-erroring on any divergence. It never edits release notes, moves tags, or clobbers assets (its argv never contains `edit` or `upload`). One caveat: an existing asset is verified by uploaded name and byte size only, so a divergent asset of identical size would pass verification. Treat published assets as immutable and forward-fix by cutting a new version rather than replacing an asset in place.

## GitHub Action direction

The current downstream install contract is a direct download of a released Toven binary, pinned by version and SHA-256 checksum — there is no required action.

A dedicated `toven-action` that installs and runs Toven inside a workflow is a candidate future mechanism to standardize that install-and-run step. It is explicitly deferred and out of scope: nothing in this repository or in rskit and gokit depends on it, and release policy stays in `toven.toml` and Toven itself. If such an action is built later, it would wrap the same version-and-checksum-pinned binary without changing release policy.

## Local workflow reproduction

When `act` is installed:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

These commands reproduce workflow structure locally but do not replace GitHub-hosted identity, signing, provenance, or release verification.
