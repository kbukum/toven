---
name: release
description: >-
    Cut a release of Toven — decide the semver bump, update the CHANGELOG, set the workspace
    version, run the full pre-release gate and supply-chain sweep, land the version commit on
    protected `main` through a reviewed PR, then dispatch the gated Release workflow whose
    `toven release publish` step creates the tag and hosted Release with per-target signed
    binaries, SBOM, and provenance. Toven ships tagged, signed binary artifacts (all crates are
    publish = false) — it does not publish to crates.io. Use when preparing or publishing a Toven
    release or checking release readiness.
user-invocable: true
---

# Releasing Toven

Toven is a **single Cargo workspace of binaries and internal crates — every crate is `publish = false`**, so a release is **not** a crates.io publish. A release is a signed, tagged **binary** release: the manually dispatched `Release` workflow ([`.github/workflows/release.yml`](../../workflows/release.yml)) builds a per-target `toven` archive for every supported target, assembles a CycloneDX SBOM and a combined `SHA256SUMS`, keyless-signs that checksum file with Sigstore/cosign, then — behind the protected `release` environment's required-reviewer gate — runs `toven release publish`, which **creates the version tag itself** and cuts the hosted GitHub Release with build provenance attested over the published `SHA256SUMS`. The whole workspace shares one version (root `Cargo.toml`); `PACKAGE_VERSION` in the `Makefile` is derived from it.

The release is **not** driven by pushing a `v*` tag. `toven release publish` creates the tag from inside the gated workflow; a tag-triggered run would race its own immutable-tag preflight, so `release.yml` is `workflow_dispatch`-only (see [`docs/self-hosting.md`](../../../docs/self-hosting.md) "Release approval pipeline"). The `Release Readiness` workflow ([`.github/workflows/release-readiness.yml`](../../workflows/release-readiness.yml)) is a **non-mutating** preview only, on PRs and `main`.

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
make act-release-readiness   # runs .github/workflows/release-readiness.yml via act
make act-supply-chain        # runs .github/workflows/supply-chain.yml via act
make release-artifacts       # packages the native-target dist/toven-<target>.<ext> archive
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

## Step 5 — Land the version commit on `main` through a reviewed PR

`main` is protected and rejects direct pushes, and `[ecosystems.rust.release]` sets `push_branch = false` for exactly this reason: the release pushes only the tag, so the version/CHANGELOG commit must reach `main` the normal way. **Do not tag by hand** — `toven release publish` creates the tag inside the gated workflow, and a manually pushed `v*` tag would race that immutable-tag preflight.

The maintainer commits the CHANGELOG + version bump on a branch and opens a PR:

```bash
git switch -c release/vX.Y.Z
git commit -am "chore: release vX.Y.Z"
git push origin release/vX.Y.Z   # then open and merge the PR into main
```

## Step 6 — Dispatch the gated Release workflow

On the merged release commit, dispatch `Release` ([`.github/workflows/release.yml`](../../workflows/release.yml)):

```bash
gh workflow run release.yml --ref main
```

The workflow builds every per-target archive (`vendored-openssl`; `aarch64-unknown-linux-gnu` via `cross`), assembles the fixed `dist/` asset set (archives + `toven-sbom.cdx.json` + `SHA256SUMS` + its keyless Sigstore signature/certificate), and preserves a mutation-free `release-preview` for the reviewers. Approve the protected `release` environment only after reviewing that preview. On approval the `publish` job runs `toven release publish --yes`, which creates the version tag and cuts the hosted Release with the assets attached, then attests build provenance over the published `SHA256SUMS`; the `verify` job re-downloads every asset and checks the Sigstore signature, checksum, and (where runnable) `--version`. CI actions stay SHA-pinned.

## Safety rules

- **Never** run destructive git commands (`reset --hard`, `checkout -- .`, `clean`) on uncommitted work without explicit permission.
- Per repo workflow, the agent prepares the branch/CHANGELOG/version edits; **the maintainer merges the PR and dispatches/approves the Release workflow**. The tag is created by `toven release publish` — never tag or push a `v*` tag by hand. Open a PR only when explicitly requested, following the PR template.
- Release tags, hosted Releases, and hosted assets are immutable create-or-verify. A partially completed release is forward-fixed with a new version and a fresh approval — never by moving a tag, editing notes, or clobbering an asset.
- Do not bump wire/protocol version constants for a release while pre-stable unless the wire shape actually changed — the umbrella and drivers build from one tree.
- Reference other-repo items with full URLs, never bare `#123`.
