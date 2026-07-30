# Release configuration

Release policy is declared under `[ecosystems.<id>.release]` and may be overridden under `[modules."<ecosystem>:<name>".release]`. Precedence is:

```text
release CLI override > module release override > ecosystem release policy > adapter default
```

`toven release plan` reports the winning version input for each module.

## Supported configuration

```toml
[ecosystems.rust.release]
strategy = "semver-cascade"
level = "auto"
dependent_version = "bump"
tag_format = "{ecosystem}/{module}@{version}"
tag_message = "release {module} {version}"
commit_message = "chore: release"
push = true
push_branch = true
remote = "origin"
branches = ["main"]
registry = "crates-io"
exclude = false
offline = false
readiness = ["clean-tree", "registry-idempotent"]

[ecosystems.rust.release.prerelease]
channels = ["alpha", "beta", "rc"]

[ecosystems.rust.release.changelog]
path = "CHANGELOG.md"
required = true

[ecosystems.rust.release.host]
forge = "github"
draft = false
assets = ["dist/core.tar.gz", "dist/SHA256SUMS"]
```

| Field | Meaning | Default |
|---|---|---|
| `strategy` | Version decision policy: `semver-cascade` (compute the next version from changes) or `manifest` (cut exactly the declared manifest version) | `semver-cascade` |
| `level` | Changed-module bump: `patch`, `minor`, `major`, or `auto` | `auto` |
| `dependent_version` | `bump` releases a dependent; `upgrade` only raises its dependency floor | `bump` |
| `tag_format` | Rust tag template; forbidden for Go | Adapter tag scheme |
| `tag_message` | Annotated-tag message template | Lightweight tag |
| `commit_message` | Release commit template | Adapter default |
| `push` | Permit release commit and tag push | `true` |
| `push_branch` | Push the release commit's branch alongside the tags; `false` pushes tags only, for a protected release branch whose commit lands through a pull request | `true` |
| `remote` | Git remote for release pushes | `origin` |
| `branches` | Allowed release branches; an empty list permits any branch | Any branch |
| `registry` | Registry selection for Rust crate publication; invalid for Go | No registry, tag-only |
| `publish` | `false` selects tag-only publication; in a per-module block it also narrows an inherited registry to tag-only | Inherit / registry-driven |
| `exclude` | Exclude the module from release planning, tagging, registry publication, and hosted releases | `false` |
| `offline` | Use tags rather than target queries for idempotency; `release status` skips registry lookups too | `false` |
| `token_env` | Name of the environment variable holding the registry publish token. The publishing adapter reads it at the toolchain boundary and forwards the credential (for cargo, as `CARGO_REGISTRY_TOKEN` on the child process, never on argv); a configured-but-absent variable fails the publish closed. `None` uses the toolchain's ambient credential | None |
| `readiness` | Named fail-closed checks | None |
| `hooks` | Reserved pre/post task references. **Rejected** when non-empty: release hooks are not yet executable — run such tasks explicitly around the release command | None |

Templates accept the documented release variables such as `{ecosystem}`, `{module}`, and `{version}`. Unknown placeholders, blank names, unsafe paths, and unsupported readiness checks fail validation.

A repository creates one release commit for all selected modules, so `commit_message` must render identically for every module in that repository. Module- or version-specific commit templates are suitable only when the repository releases one module at a time.

A pushing release normally updates both the release commit's branch and the release tags on the remote. When the release branch is protected and rejects direct pushes, set `push_branch = false`: the release then pushes only the tags and leaves the branch ref untouched, and the version/CHANGELOG commit reaches the branch through the normal reviewed pull-request flow. `push_branch` is per-repository like the other push settings — every module releasing in one repository must agree on it.

## Version and cascade policy

`level = "auto"` currently resolves a normal change to a patch and a known breaking signal to a minor bump. Use explicit configuration or CLI overrides when a major bump is required.

`dependent_version = "bump"` is the safe release default: when a released dependency raises a requirement floor, the dependent receives its own release. `upgrade` changes the dependency requirement without releasing the dependent and should be selected only when the ecosystem and repository intentionally permit that state.

### Version strategies

`strategy` selects **how the next version is decided**. The rest of the release flow — change detection, dependency cascade, idempotency, tag, publish — is identical for both strategies; only the version-decision node reads `strategy`.

- **`semver-cascade`** (default) — the next version is **computed**: the baseline tag plus the detected changes resolve to a patch, minor, or major bump, a pending prerelease is finalized on a stable bump, and raised dependency floors cascade into dependents. Compose a prerelease channel with `--pre <channel>`.
- **`manifest`** — the next version is **declared**: Toven cuts exactly `v${manifest version}` when the declared version is strictly ahead of the last release tag, and **fails closed** with a typed error when the declared version is equal to or behind the baseline ("bump the manifest version before releasing"). The reviewed version/CHANGELOG pull request *is* the version decision, so the tag equals the workspace version by construction. Because the channel already lives in the declared version string, `--pre` combined with `manifest` is a typed usage error. Explicit argv (`--set-version`/`--patch`/`--minor`/`--major`) still wins over either strategy.

Worked example — baseline release tag versus the declared `Cargo.toml` version:

| Baseline tag | Declared `Cargo.toml` | `semver-cascade` | `manifest` |
|---|---|---|---|
| `0.1.0-alpha.1` | `0.1.0-alpha.1` | `0.1.0` | fail closed (not ahead) |
| `0.1.0-alpha.1` | `0.1.0-alpha.2` | `0.1.0` | `0.1.0-alpha.2` |
| `0.1.0-alpha.2` | `0.1.0` | `0.1.0` | `0.1.0` (finalize, declared) |
| `0.1.0` | `0.1.1` | `0.1.1` | `0.1.1` |
| `0.1.0` | `0.1.0-alpha.2` | — | fail closed (behind) |

`manifest` is what lets a workspace cut successive `0.1.0-alpha.2`, `-alpha.3` prereleases from a curated `Cargo.toml`, where `semver-cascade` would always compute past the declared prerelease to the finalized version.

## Prereleases

Only declared channels can be selected:

```toml
[ecosystems.go.release.prerelease]
channels = ["alpha", "rc"]
```

```bash
toven release publish --dry-run --pre alpha
```

An undeclared channel is rejected. The channel becomes part of the semantic version and therefore part of the Rust or Go tag. The `branch_channels` map binds a release branch to the channel it cuts, so releasing from a `next` branch can imply a `beta` train without a per-run flag: when no explicit `--pre` is given, the checked-out branch selects the channel. An explicit `--pre <channel>` always wins over the mapping, a branch not present in the map (or a detached HEAD) cuts a stable release, and every mapped channel must be one of the declared `channels`.

## Changelogs

```toml
[ecosystems.rust.release.changelog]
path = "CHANGELOG.md"
required = true
```

When `required = true`, every directly changed release unit must have a documented entry in the configured changelog before the release proceeds. The check is file-backed: Toven resolves the project-relative `path` (default `CHANGELOG.md`), reads it, and requires a non-empty [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) `## [Unreleased]` section — at least one bullet or prose line under the heading, not just empty `### Added`/`### Fixed` subsections. A missing, unreadable, unsafe, or undocumented changelog fails validation before any mutation. Modules selected only through a dependency cascade are exempt; their release reason is the deterministic dependency-cascade explanation carried in the plan. Toven verifies the changelog but never rewrites it; authoring the `[Unreleased]` entry stays a maintainer action.

## Readiness

```toml
[ecosystems.rust.release]
readiness = ["clean-tree", "registry-idempotent"]
```

`clean-tree` and `registry-idempotent` are currently executable. `registry-idempotent` only evaluates registry-published modules; tag-only and excluded modules are ignored by that registry check. A protected publication policy should configure both for registry releases and at least `clean-tree` for tag-only releases.

## Hosted GitHub Releases

```toml
[ecosystems.rust.release.host]
forge = "github"
draft = false
assets = ["dist/core.tar.gz", "dist/SHA256SUMS"]
```

Asset paths are project-relative exact paths; glob expansion is not implemented. An unset `prerelease` flag derives from the selected version: a version carrying a prerelease identifier (`0.1.0-alpha.1`) marks the hosted Release as a prerelease, whether it came from `--pre` or from the version the module already declares. An unset `notes` value uses the planner's changelog summary.

One release tag is one hosted Release. A `tag_format` that omits the module — `v{version}` for a workspace that shares a single version — maps every module onto the same tag, so those modules collapse into a single hosted Release whose assets and notes are the deduplicated union of the contributing modules, and the shared tag itself is created once for the whole release train. Modules sharing a tag but disagreeing on `draft` or `prerelease`, or rendering different tag annotations (`tag_message`), are rejected as a configuration error before any mutation.

Hosted Release rehearsal is mutation-free. Real hosted Release creation currently runs only after a pushing `release publish`, not after `release tag`.

## Registry and tag-only policy

The contract distinguishes:

| Policy | Rust | Go |
|---|---|---|
| Registry | Publish the crate, then verify the immutable registry version | Not applicable |
| Tag-only | Create the immutable tag without registry publication | The tag is the release |
| Excluded module | Do not version, tag, publish, or host | Do not version or tag |

Toven resolves publication into a typed policy before planning:

- `registry = "crates-io"` selects Rust registry publication.
- No `registry` selects tag-only publication; Rust crates are not published by default.
- `publish = false` is an explicit tag-only declaration; in the same block it requires no `registry`, and in a per-module override it narrows an inherited ecosystem registry to tag-only for that one module.
- `exclude = true` removes the module from release planning entirely; in a per-module override it also narrows an inherited registry to excluded for that one module.

Contradictions are rejected **within a single block**: Go cannot declare a registry, a `registry` target cannot be combined with `publish = false` in the same block, and an excluded module cannot declare registry publication or hosted assets. These same-block rules do not block a more-specific **per-module override from narrowing an inherited policy**: a module that inherits a registry from its ecosystem may set `publish = false` to become tag-only, or `exclude = true` to drop out of the release, without tripping a contradiction. The override merges field-by-field and the resolved policy is recomputed from the merged fields (`exclude` wins, then `publish = false`, then `registry`), so the per-module block that narrows an inherited registry never re-triggers the same-block contradiction check. Registry-published Rust crates are packaged and published during `release publish`; tag-only modules are versioned and tagged but never sent to a package registry.

For Go test-only and benchmark modules, the policy is intentionally explicit: use tag-only release policy or `exclude = true`; path and name heuristics are forbidden. Go registry publication is rejected because Go releases are immutable Git tags.

## Signing

```toml
[ecosystems.rust.release.sign]
enabled = true
# signer = "my-key-ref"   # optional; omit for the keyless Sigstore default
identity = "https://github.com/OWNER/REPO/.github/workflows/release.yml@.*"
issuer = "https://token.actions.githubusercontent.com"
```

Signing is **off by default** and, when enabled, executable by `toven release sign`: it produces a detached signature and certificate over the `SHA256SUMS` manifest with cosign, writing them to the declared `SHA256SUMS.sig`/`SHA256SUMS.pem` assets. With no `signer`, the keyless Sigstore default is used — the signing identity comes from the ambient OIDC token (GitHub Actions), and no private key is stored in Toven configuration; `signer` names a non-secret key/identity selection when a keyed signer is intended. A configured-but-unavailable signer, or a signer failure, fails the release closed.

`identity` and `issuer` are the keyless **verification** inputs consumed by `toven release verify --download`: the `certificate-identity-regexp` a downloaded signature's certificate must match (the release workflow ref) and the `certificate-oidc-issuer` it must chain to. They are not secrets, and they let any consumer verify against *their own* workflow identity rather than a hard-coded one. Signing must be enabled for a `signer` to be set, and a blank `signer`, `identity`, or `issuer` fails validation.

## Artifact assembly and verification

Under a hosted-release policy, the fixed `host.assets` set is produced end to end by engine verbs — no external packaging, checksum, or signing scripts:

- `toven release package --target <triple>` archives an already-built binary (`target/<triple>/release/<binary>`, or an explicit `--binary` path) into its declared per-target archive asset (`.tar.gz`, or `.zip` for a `*windows*` triple, with the `.exe` suffix recorded). It fails closed when the declared asset or the built binary is missing.
- `toven release sbom` writes the CycloneDX SBOM and stages it into the declared `*.cdx.json` asset.
- `toven release checksums` digests every declared archive and the SBOM into the `SHA256SUMS` manifest asset, in declared order, using SHA-256.
- `toven release sign` signs `SHA256SUMS` into its `.sig`/`.pem` sidecar assets (see Signing).
- `toven release verify` checks the declared archives: in **local** mode it presence-checks every declared archive and, unless `--no-run`, extracts and asserts each reports the decided version; with `--download` it fetches the archives plus `SHA256SUMS` and its signature from the hosted release and verifies them in a hard fail-closed order — the Sigstore signature on `SHA256SUMS` first, then each archive's checksum, then extraction and version check.

Every verb is non-mutating with respect to git history, emits typed JSONL under `--output jsonl`, and fails closed on a missing or mismatched input. CI provisions the external tools (cosign, cargo-cyclonedx) and holds the human approval gate; Toven drives them.

## Safety

- Preview commands must not mutate manifests, commits, tags, registries, or hosted Releases.
- Real publication requires `--yes`, an allowed branch, and a clean tree.
- Release tags, registry versions, hosted Releases, and hosted assets are immutable.
- Tokens remain in environment variables or credential stores: `token_env` names the variable; the secret is read only at the publishing toolchain boundary and never appears on argv, in a log, or in engine memory.
- Settings that name capabilities Toven does not execute — release `hooks` — are rejected with actionable errors rather than silently ignored.
- Partial publication is recovered by a newly previewed and approved forward fix.

The clean-tree guardrail has no bypass, and hosted GitHub Releases are immutable create-or-verify: an existing Release is verified byte-identical to the intended one or the run fails with a conflict, never edited in place.
