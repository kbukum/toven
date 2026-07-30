---
name: release
description: >-
    Cut a release of Toven — decide the semver bump, update the CHANGELOG, set the workspace
    version, run the full pre-release gate and supply-chain sweep, then tag so CI publishes the
    signed source artifact, SBOM, and provenance. Toven ships tagged, signed build artifacts (all
    crates are publish = false) — it does not publish to crates.io. Use when preparing or
    publishing a Toven release or checking release readiness.
user-invocable: true
---

# Releasing Toven

Toven is a **single Cargo workspace of binaries and internal crates — every crate is `publish = false`**, so a release is **not** a crates.io publish. A release is a signed, tagged build: pushing a `v*` tag drives the `Release Readiness` workflow ([`.github/workflows/release-readiness.yml`](../../workflows/release-readiness.yml)) to build the release binaries, produce the source tarball + `SHA256SUMS`, generate a CycloneDX SBOM, and attach build provenance. The whole workspace shares one version (root `Cargo.toml`); `PACKAGE_VERSION` in the `Makefile` is derived from it.

The engineering baseline still applies ([`docs/engineering.md`](../../../docs/engineering.md)): supply chain pinned and clean, `Cargo.lock` committed, artifacts signed with SBOM + provenance.

## Prerequisites

- Listed in [`MAINTAINERS.md`](../../../MAINTAINERS.md) with push access to `kbukum/toven`.
- On `main`, clean working tree, submodules initialized (`git submodule update --init --recursive`).
- `git`, `gh`, `cargo`, `cargo-nextest`, `cargo-deny`, and (for local artifact/signing checks) `act`, `cosign` on `$PATH`.

## Step 1 — Full pre-release gate

A release is the one time to run the **complete** gates rather than the affected set:

```bash
make check              # fmt-check + lint + test + structure + doc + deny + release-dry-run
make release-dry-run    # cargo metadata sanity + full --release --all-features build
make deny               # cargo-deny: advisories, bans, licenses, sources
```

Then dry-run the release-readiness and supply-chain workflows locally, and rebuild the artifacts:

```bash
make act-release-readiness   # runs .github/workflows/release-readiness.yml via act (includes the release-archive package job)
make act-supply-chain        # runs .github/workflows/supply-chain.yml via act
```

Package a native-target archive directly with the engine verb (the workflow runs the same verb per matrix target):

```bash
host="$(rustc -vV | sed -n 's/host: //p')"
cargo build --locked --release -p toven --target "$host"
cargo run --locked -p toven -- release package --target "$host"   # writes dist/toven-<host>.<ext>
```

Also run the `review` project audit in a fresh agent before a release. Treat green gates as necessary but not sufficient.

## Step 2 — Decide the version

```bash
git tag --sort=-v:refname | head -1
git log --oneline $(git describe --tags --abbrev=0 2>/dev/null)..HEAD
```

Toven follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While pre-1.0 (`0.x`): a breaking change in the `[Unreleased]` CHANGELOG section bumps **MINOR**; otherwise **PATCH**. Pre-stable, backward compatibility is not yet a goal — the baseline prefers root-cause redesigns.

## Step 3 — Update the CHANGELOG

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); entries accumulate under `[Unreleased]`.

1. Replace `## [Unreleased]` with `## [vX.Y.Z] - YYYY-MM-DD`.
2. Add a fresh empty `## [Unreleased]` section above it (with the standard `### Added / Changed / …` headings).
3. If the `[vX.Y.Z]` section is empty, **refuse to release** — nothing to ship.
4. Update the link references at the bottom if present.

## Step 4 — Set the workspace version

Bump `version` in the root `Cargo.toml` (the whole workspace shares it and `PACKAGE_VERSION` derives from it), then let the build refresh the lockfile — never hand-edit `Cargo.lock`, and never `cargo update` (it would pull unreviewed dependency bumps into the release):

```bash
make release-dry-run       # rebuilds at the new version and syncs the workspace-member versions in Cargo.lock
```

Keep the resulting `Cargo.lock` committed.

## Step 5 — Tag and let CI sign and create attestations

The maintainer commits the CHANGELOG + version bump, then tags. Pushing the `v*` tag triggers `Release Readiness`, which builds the artifacts, generates the SBOM, and (tag-only) attaches build provenance for `dist/SHA256SUMS`:

```bash
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

Then create the GitHub release with notes from the `[vX.Y.Z]` CHANGELOG section and attach the signed source tarball, `SHA256SUMS`, and SBOM. CI actions stay SHA-pinned.

## Safety rules

- **Never** run destructive git commands (`reset --hard`, `checkout -- .`, `clean`) on uncommitted work without explicit permission.
- Per repo workflow, the agent prepares the branch/CHANGELOG/version edits; **the maintainer commits, pushes, and tags**. Open a PR only when explicitly requested, following the PR template.
- Do not bump wire/protocol version constants for a release while pre-stable unless the wire shape actually changed — the umbrella and drivers build from one tree.
- Reference other-repo items with full URLs, never bare `#123`.
