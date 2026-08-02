# Self-hosting and CI

Toven uses its own planner for mapped development and release previews. The Makefile remains the stable local and CI entry point.

## Binary selection

By default, Make targets run the freshly built workspace binary:

```makefile
TOVEN ?= cargo run --quiet --locked -p toven --
```

An installed and verified released binary can be selected explicitly:

```bash
make TOVEN=toven check
```

CI must pin a Toven version and checksum. It must not use an unversioned latest-release URL.

## Canonical gate

```bash
make check
```

The gate includes formatting, linting, nextest, doctests, structure checks, rustdoc, dependency policy, and release build readiness. It is **Toven-driven end to end**: every gate resolves through a Toven task, with one deliberate, documented exception.

| Gate | Execution |
|---|---|
| Lint, nextest, rustdoc, release build | Toven task |
| Rust doctests | Toven `doctest` task (`cargo test --doc`) |
| Dependency policy (cargo-deny) | Toven `deny` task |
| Declare-only structure (ast-grep) | Toven `structure` task (command ecosystem) |
| Documentation build (mdbook) | Toven `docs-build` task (command ecosystem) |
| rustfmt workspace check | Native Cargo — the sole documented exception |

`fmt-check` stays native on purpose: `make check` gates the whole workspace in a single fast `cargo fmt --all --check` pass, and the granular `format`/`format-check` rust tasks remain available through Toven. Every other gate — including the ones that were once bespoke Makefile recipes with `command -v` tool guards (`structure`, `deny`, doctests, `docs-build`) — now runs through `toven run <task>`, and the tool-presence guards are replaced by a single [`doctor`](commands/doctor.md) audit.

`make doctor` (`toven doctor --ensure`) is the single source of truth for required tooling: it walks the resolved task graph, reports every tool its tasks need (`cargo`, `ast-grep`, `mdbook`), and fails closed on a gap. CI provisions against that report and runs it as a fail-fast gate before the rest of the surface, so a missing tool surfaces once, up front, rather than mid-gate.

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
4. Cut the release version from the reviewed manifest. Toven's `[ecosystems.rust.release]` sets `strategy = "manifest"`, so the engine cuts exactly `v${Cargo.toml}` when the declared version is strictly ahead of the last release tag, and fails closed otherwise. The version/CHANGELOG pull request *is* the version decision, so the tag the engine cuts equals the version each `build`-job binary was packaged from, by construction — there is no separate preflight to assert they match.
5. Preserve machine-readable stdout and generated evidence as review artifacts.
6. Require a protected-environment approval tied to the exact commit and preview.
7. Recheck the branch, clean tree, selected version, and absence of conflicting immutable results.
8. Publish with least-privilege permissions.
9. Download and verify every registry artifact, tag, binary, checksum, signature, SBOM, provenance record, and hosted asset.

Human release tables and JSONL use stdout. Warnings and mutation progress use stderr. Automation should parse only stdout when `--output jsonl` is selected and retain stderr as diagnostics.

Because Toven's own `main` is protected, its `[ecosystems.rust.release]` sets `push_branch = false`: the approved publish job pushes only the release tag, and the version/CHANGELOG commit lands on `main` through the normal reviewed pull-request flow.

```bash
toven release publish --dry-run --output jsonl > release-preview.jsonl
```

## Toven's binary releases

Toven's releases are hosted binary releases, not crates.io publications. Every workspace crate remains `publish = false`. The first release was `v0.1.0-alpha.1`; later alpha prereleases follow the same contract.

The release matrix is:

| Archive target | Format | Archive name |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `.tar.gz` | `toven-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` | `toven-aarch64-unknown-linux-gnu.tar.gz` |
| `x86_64-apple-darwin` | `.tar.gz` | `toven-x86_64-apple-darwin.tar.gz` |
| `aarch64-apple-darwin` | `.tar.gz` | `toven-aarch64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` | `toven-x86_64-pc-windows-msvc.zip` |

Archive names are fixed and never embed the version: `toven.toml`'s `[ecosystems.rust.release.host]` `assets` list is a set of exact, non-templated project-relative paths (globbing and version placeholders are not implemented — see `crates/toven-ports/src/config/release/host.rs`), so the same static list must resolve on every release. The version lives in the release tag and Release title instead.

Every archive contains one directly runnable binary. The hosted Release also contains a CycloneDX SBOM (`toven-sbom.cdx.json`), a combined `SHA256SUMS` covering every archive and the SBOM, that file's keyless Sigstore/cosign signature and certificate (`SHA256SUMS.sig`, `SHA256SUMS.pem`), and a separate GitHub build provenance attestation (not a listed asset; verify it with `gh attestation verify`).

`.github/workflows/release.yml` builds this matrix, assembles the fixed `dist/` file set, and runs `toven release publish` behind the protected `release` environment's required-reviewer approval. It is dispatched manually (`workflow_dispatch`) rather than triggered by a `v*` tag push, because `toven release publish` creates that tag itself; a tag-triggered run would race its own immutable-tag preflight. Every asset in the fixed set is produced by an engine verb, not a bash script: the `build` job packages each target with `toven release package --target <triple>`, and the `assemble` job stages the SBOM (`toven release sbom`), writes the combined `SHA256SUMS` over every archive and the SBOM (`toven release checksums`), signs it with the keyless Sigstore/cosign default (`toven release sign`), and presence-checks the whole declared asset set (`toven release verify --no-run`). CI still provisions the tools (cosign, cargo-cyclonedx, `cross`) and holds the approval gate; Toven drives them. Every target builds with the `vendored-openssl` feature, so rskit-git's embedded git2 backend links a source-built OpenSSL (libgit2 is already vendored) and the released binaries carry no host OpenSSL dependency. The cross-compiled `aarch64-unknown-linux-gnu` target builds through `cross` for a matching glibc. Build provenance is attested in the approved `publish` job, over the subjects of the published `SHA256SUMS`, so an attestation exists only for artifacts that were actually approved and published.

After publish, the `verify` job downloads every published asset and runs `toven release verify --download`, which verifies the keyless Sigstore signature on `SHA256SUMS` first, then checksum-verifies every published archive against the now-trusted manifest. The keyless `certificate-identity-regexp` and `certificate-oidc-issuer` it matches against come from `[ecosystems.rust.release.sign]` in `toven.toml`, not a hard-coded string. It runs with `--no-run`: the verb verifies the whole declared asset set at once, and no single hosted runner can execute all five targets (two Linux, two macOS, one Windows), so post-publish verification is signature- and checksum-based across the whole set.

## Dogfooding the binary Toven produces

Toven proves its release platform on itself, driving Toven's own work with the binary Toven builds and packages — not a source `cargo run` and not a downloaded release.

`.github/workflows/self-canary.yml` is that proof. On every push to `main` (and on manual dispatch) it builds the release binary, packages it into its declared `dist/` archive with `toven release package`, extracts that self-generated binary, and then runs Toven's whole toven-driven surface through it: module discovery, `plan check`, the `doctor --ensure` required-tool audit, the full `make check` gate surface (`lint`, `test` — nextest plus the `doctest` task — `doc`, `structure`, `deny`, `docs-build`, `coverage`, `affected`) via `make TOVEN=<binary>`, the `format-check` and `vuln` mapped tasks that `make check` otherwise gates natively, and the full mutation-free release preview (`release plan`/`status`/`readiness`/`sbom`/`depgraphs` and `publish --dry-run`). Nothing after packaging uses a source build, so a green run means the artifact Toven ships can perform every job Toven is built for. It needs no published release: the canary self-generates the binary it dogfoods.

Two related but distinct checks cover the *released* artifact rather than the self-generated one, so the self-canary does not duplicate them:

- `release.yml`'s `verify` job downloads every published asset and re-verifies it with `toven release verify --download` (signature on `SHA256SUMS`, then each archive's checksum).
- `scripts/install-toven.sh <version> [install-dir]` is the reference downstream install contract a consumer (or another repository) uses: it pins an immutable release tag, downloads the matching per-target archive and `SHA256SUMS`, verifies the keyless Sigstore signature on `SHA256SUMS` when `cosign` is present, checksum-verifies the archive before extraction, and installs `toven`. It never uses an unpinned latest-release URL and passes no secret on argv.

## Artifact retention

Two kinds of artifacts have very different lifetimes: transient workflow-run artifacts uploaded for review and debugging, and the permanent assets attached to a published hosted Release.

Workflow-run artifacts are ephemeral. They exist only to make a run reviewable and are pruned by GitHub once their retention window elapses:

| Artifact | Workflow | Job | Retention |
|---|---|---|---|
| `release-preview` (preview `release-preview.jsonl`, SBOM, dependency graphs) | `release.yml` | preview | 14 days |
| `release-archive-<target>` (one packaged per-target archive) | `release.yml` | per-target build | 14 days |
| `release-dist` (assembled `dist/` with every archive, SBOM, `SHA256SUMS`, signature, certificate) | `release.yml` | assemble | 14 days |
| `release-publish-record` (`release-publish.jsonl` mutation record) | `release.yml` | publish | 90 days |
| `release-archive` (a locally built native-target archive) | `release-readiness.yml` | build | 7 days |

The publish record is kept longest (90 days) because it is the machine-readable record of what an approved mutation actually did. Preview and staging artifacts (14 days) outlive a normal review-and-approve cycle without accumulating indefinitely, and readiness artifacts (7 days) are the shortest-lived because they carry no immutable outcome and are regenerated on every readiness run.

None of these windows affect the release itself. The binaries, checksums, signature, certificate, and SBOM attached to the published hosted Release, the immutable version tag, and the separate build provenance attestation are permanent parts of the Release and are not governed by `retention-days`. They persist until the Release is deleted, which the immutable create-or-verify policy below forbids as a repair mechanism. Consumers pin against those published assets, never against a transient workflow-run artifact.

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
