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

`fmt-check` stays native on purpose: `make check` gates the whole workspace in one fast `cargo fmt --all --check` pass. The granular `format`/`format-check` Rust tasks are still available through Toven. Every other gate runs through `toven run <task>`, and per-task `command -v` tool guards are replaced by a single [`doctor`](commands/doctor.md) audit.

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
4. Cut the release version from the reviewed manifest. `[ecosystems.rust.release]` sets `strategy = "manifest"`, so the engine cuts exactly `v${Cargo.toml}` when the declared version is strictly ahead of the last release tag, and fails closed otherwise. The version/CHANGELOG pull request *is* the version decision: the tag the engine cuts equals the version each `build`-job binary was packaged from, so no separate preflight is needed to assert they match.
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

Toven's releases are hosted binary releases, not crates.io publications. Every workspace crate remains `publish = false`, and alpha prereleases use the `v0.1.0-alpha.N` tag contract.

The release matrix is:

| Archive target | Format | Archive name |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `.tar.gz` | `toven-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` | `toven-aarch64-unknown-linux-gnu.tar.gz` |
| `x86_64-apple-darwin` | `.tar.gz` | `toven-x86_64-apple-darwin.tar.gz` |
| `aarch64-apple-darwin` | `.tar.gz` | `toven-aarch64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` | `toven-x86_64-pc-windows-msvc.zip` |

Archive names are fixed and never embed the version. `[ecosystems.rust.release.host].assets` is a set of exact, non-templated project-relative paths (no globbing or version placeholders — see `crates/toven-ports/src/config/release/host.rs`), so the same static list resolves on every release. The version lives in the release tag and Release title.

Every archive contains one directly runnable binary. The hosted Release also carries a CycloneDX SBOM (`toven-sbom.cdx.json`), a combined `SHA256SUMS` over every archive and the SBOM, that file's keyless Sigstore/cosign bundle (`SHA256SUMS.bundle`), and a separate GitHub build provenance attestation (not a listed asset — verify it with `gh attestation verify`).

`.github/workflows/release.yml` builds this matrix and publishes it behind the protected `release` environment's required-reviewer approval. It is dispatched manually (`workflow_dispatch`), never triggered by a `v*` tag push: `toven release publish` creates the tag itself, and a tag-triggered run would race its own immutable-tag preflight. Every asset is produced by an engine verb, not a bash script:

| Job | What it runs | Result |
|---|---|---|
| `build` (per target) | `toven release package --target <triple>` | one packaged archive per target |
| `assemble` | `toven release sbom` → `checksums` → `sign` → `verify --no-run` | staged SBOM, combined `SHA256SUMS`, its signature, a presence-checked asset set |
| `publish` | `toven release publish --yes`, then `actions/attest-build-provenance`, then `toven release provenance` | the tag, the hosted Release, a build-provenance attestation cut by the trusted builder over `SHA256SUMS`, and Toven's verification that it exists |
| `verify` | `toven release verify --download` | signature- and checksum-verifies every published asset |
| `publish-packages` | `.github/workflows/publish-packages.yml` (called with the published tag) | renders the Homebrew formula and Scoop manifest from the released `SHA256SUMS` and pushes them to the tap/bucket repos |

CI provisions the tools (cosign, cargo-cyclonedx, `cross`) and holds the approval gate; Toven drives them. Every target builds with the `vendored-openssl` feature, so rskit-git's embedded git2 backend links a source-built OpenSSL and the released binaries carry no host OpenSSL dependency. The `aarch64-unknown-linux-gnu` target cross-compiles through `cross` for a matching glibc.

`verify` runs with `--no-run`: no single hosted runner can execute all five targets (two Linux, two macOS, one Windows), so post-publish verification is signature- and checksum-based across the whole set. The keyless `certificate-identity-regexp` and `certificate-oidc-issuer` it matches come from `[ecosystems.rust.release.sign]` in `toven.toml`, not a hard-coded string.

The `publish-packages` job calls `.github/workflows/publish-packages.yml` as a reusable workflow after `publish` and `verify` succeed, rather than binding it to `release: published`. GitHub does not emit event-triggering webhooks for actions taken with the default `GITHUB_TOKEN` (recursion prevention), so a Release the pipeline creates would never fire that event — the package managers would silently stay behind. Calling it in-pipeline keeps the tap and bucket in lock-step with every published release; it is a no-op unless a `HOMEBREW_TAP_TOKEN` secret is configured, and `workflow_dispatch` remains for manual backfill.

## Dogfooding the binary Toven produces

Toven proves its release platform on itself. `.github/workflows/self-canary.yml` runs Toven's own work with the binary Toven builds and packages — not a source `cargo run`, and not a downloaded release. It runs on every push to `main` and needs no published release: it self-generates the binary it dogfoods.

The canary builds the release binary, packages it into its `dist/` archive with `toven release package`, extracts that binary, and runs everything below **through it**. Nothing after packaging uses a source build, so a green run means the shipped artifact can do every job Toven is built for:

- **Discovery and planning** — `modules`, `plan check`.
- **Required-tool audit** — `doctor --ensure`.
- **The full `make check` gate surface** via `make TOVEN=<binary>` — `lint`, `test` (nextest + doctests), `doc`, `structure`, `deny`, `docs-build`, `coverage`, `affected` — plus the `format-check` and `vuln` mapped tasks.
- **Read-only verbs** — `explain`, `init --print` (asserted to leave the tree clean), `completions`, `cache path`.
- **Mutation-free release previews** — `release plan`/`status`/`readiness`/`sbom`/`depgraphs` and `publish --dry-run`.

**The assembly path, proven end to end.** `release checksums`/`sign`/`verify`/`provenance` are policy over the *complete* declared asset set — one combined `SHA256SUMS` over every per-target archive and the SBOM. There is no single-target release, so a `build` matrix first produces every archive exactly as `release.yml` does. An `assemble` job then extracts the Linux archive (itself the self-generated binary) and drives the assembly through it, mutation-free:

- `release sbom` stages the CycloneDX SBOM.
- `release checksums` writes the combined manifest.
- `release verify --no-run` presence-checks the whole set without running any target. Local verify is presence-only; signature and checksum are verified only in the hosted `--download` mode.
- `release provenance --dry-run` reports whether an attestation already exists for each subject, without ever creating one.

`release sign` is exercised too, but only on manual dispatch. Keyless Sigstore/cosign signing writes a permanent, public Rekor transparency-log entry on every run — a real external side effect (it touches no tag, release, or remote). Gating it to `workflow_dispatch` proves the sign path on demand without adding a canary entry on every merge.

Two related but distinct checks cover the *released* artifact rather than the self-generated one, so the self-canary does not duplicate them:

- `release.yml`'s `verify` job downloads every published asset and re-verifies it with `toven release verify --download` (signature on `SHA256SUMS`, then each archive's checksum).
- `scripts/install.sh` is the reference downstream install contract a consumer (or another repository) uses. Run with no arguments it installs the latest release; passed `--version <tag>` (as CI must) it pins an immutable release tag. Either way it downloads the matching per-target archive and `SHA256SUMS`, verifies the keyless Sigstore signature on `SHA256SUMS` when `cosign` is present, checksum-verifies the archive before extraction, and installs `toven`. In CI, pin both the version and the script URL to a tag so no unpinned latest-release URL enters the pipeline; no secret is passed on argv.

## Artifact retention

Two kinds of artifacts have very different lifetimes: transient workflow-run artifacts uploaded for review and debugging, and the permanent assets attached to a published hosted Release.

Workflow-run artifacts are ephemeral. They exist only to make a run reviewable and are pruned by GitHub once their retention window elapses:

| Artifact | Workflow | Job | Retention |
|---|---|---|---|
| `release-preview` (preview `release-preview.jsonl`, SBOM, dependency graphs) | `release.yml` | preview | 14 days |
| `release-archive-<target>` (one packaged per-target archive) | `release.yml` | per-target build | 14 days |
| `release-dist` (assembled `dist/` with every archive, SBOM, `SHA256SUMS`, signature, certificate) | `release.yml` | assemble | 14 days |
| `release-publish-record` (`release-publish.jsonl` + `release-provenance.jsonl` mutation/provenance record) | `release.yml` | publish | 90 days |
| `release-archive` (a locally built native-target archive) | `release-readiness.yml` | build | 7 days |
| `self-canary-release-preview` (`release-preview.jsonl` publish rehearsal) | `self-canary.yml` | dogfood | 14 days |
| `self-canary-archive-<target>` (one packaged per-target archive) | `self-canary.yml` | per-target build | 14 days |
| `self-canary-release-dist` (assembled `dist/` + provenance preview, through the self-generated binary) | `self-canary.yml` | assemble | 14 days |

The publish record is kept longest (90 days): it is the machine-readable record of what an approved mutation did. Preview and staging artifacts (14 days) outlive a normal review-and-approve cycle without piling up. Readiness artifacts (7 days) are shortest-lived — they carry no immutable outcome and regenerate on every run.

These windows never affect the release itself. The published binaries, checksums, signature, certificate, SBOM, the immutable version tag, and the build provenance attestation are permanent parts of the hosted Release, not governed by `retention-days`. They persist until the Release is deleted — which the immutable create-or-verify policy below forbids as a repair. Consumers pin against published assets, never against a transient workflow-run artifact.

## Immutability and recovery

Release tags, registry versions, hosted Releases, and approved assets are immutable. CI must fail if any intended output already exists with different content. A partially completed release is not repaired by moving tags, deleting registry versions, editing release notes, or clobbering assets. Correct the source or workflow, select a forward-fix version, regenerate the preview, and obtain approval again.

The current GitHub host adapter is **immutable create-or-verify**: it creates the Release, or — if one already exists — reads it back and verifies it matches exactly, hard-erroring on any divergence. It never edits release notes, moves tags, or clobbers assets (its argv never contains `edit` or `upload`). One caveat: an existing asset is verified by uploaded name and byte size only, so a divergent asset of identical size would pass. Treat published assets as immutable and forward-fix by cutting a new version.

## GitHub Action

The reusable action at `.github/actions/toven` wraps the install-and-run step into one pinned `uses:` line. It reuses `scripts/install.sh` bundled at the action's pinned commit — no second download or verification path — and adds a runner tool-cache, cosign auto-install, unchanged argument forwarding, and typed `toven`/`version`/`cache-hit` outputs. It never publishes: release policy stays in `toven.toml` and Toven itself, behind each repository's approved release environment.

Consumers pin both the action (by commit SHA) and the binary (by `version`):

```yaml
- uses: kbukum/toven/.github/actions/toven@<commit-sha> # v0.1.0-alpha.3
  with:
    version: v0.1.0-alpha.3
    args: modules
```

The direct download below remains fully supported for repositories that prefer an explicit install step; the action reproduces it rather than replacing it.

## Local workflow reproduction

When `act` is installed:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

These commands reproduce workflow structure locally but do not replace GitHub-hosted identity, signing, provenance, or release verification.
