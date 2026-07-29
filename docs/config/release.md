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
| `strategy` | Version and cascade policy; currently `semver-cascade` | `semver-cascade` |
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
| `token_env` | Reserved. **Rejected** for registry-published modules: not yet honored — the credential reaches the publishing toolchain through its ambient environment (e.g. `CARGO_REGISTRY_TOKEN` for cargo) | None |
| `readiness` | Named fail-closed checks | None |
| `hooks` | Reserved pre/post task references. **Rejected** when non-empty: release hooks are not yet executable — run such tasks explicitly around the release command | None |

Templates accept the documented release variables such as `{ecosystem}`, `{module}`, and `{version}`. Unknown placeholders, blank names, unsafe paths, and unsupported readiness checks fail validation.

A repository creates one release commit for all selected modules, so `commit_message` must render identically for every module in that repository. Module- or version-specific commit templates are suitable only when the repository releases one module at a time.

A pushing release normally updates both the release commit's branch and the release tags on the remote. When the release branch is protected and rejects direct pushes, set `push_branch = false`: the release then pushes only the tags and leaves the branch ref untouched, and the version/CHANGELOG commit reaches the branch through the normal reviewed pull-request flow. `push_branch` is per-repository like the other push settings — every module releasing in one repository must agree on it.

## Version and cascade policy

`level = "auto"` currently resolves a normal change to a patch and a known breaking signal to a minor bump. Use explicit configuration or CLI overrides when a major bump is required.

`dependent_version = "bump"` is the safe release default: when a released dependency raises a requirement floor, the dependent receives its own release. `upgrade` changes the dependency requirement without releasing the dependent and should be selected only when the ecosystem and repository intentionally permit that state.

## Prereleases

Only declared channels can be selected:

```toml
[ecosystems.go.release.prerelease]
channels = ["alpha", "rc"]
```

```bash
toven release publish --dry-run --pre alpha
```

An undeclared channel is rejected. The channel becomes part of the semantic version and therefore part of the Rust or Go tag. The reserved `branch_channels` map (a release branch to the channel it cuts) is **rejected** when non-empty: branch-driven prereleases are not yet executable — select the channel explicitly with `--pre`.

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
```

Artifact signing is **not yet executable** by `toven release`: the typed configuration validates signing intent and an optional non-secret signer identifier, and release resolution then rejects `enabled = true` with an actionable error rather than letting a maintainer believe artifacts ship signed. Keep signing in the native CI gate until Toven executes it. Toven's own distribution contract uses keyless Sigstore/cosign signing in GitHub Actions with OIDC; no private key is stored in Toven configuration.

## Safety

- Preview commands must not mutate manifests, commits, tags, registries, or hosted Releases.
- Real publication requires `--yes`, an allowed branch, and a clean tree.
- Release tags, registry versions, hosted Releases, and hosted assets are immutable.
- Tokens remain in environment variables or credential stores.
- Settings that name capabilities Toven does not execute — release `hooks`, `sign.enabled = true`, `prerelease.branch_channels`, and `token_env` on a registry-published module — are rejected with actionable errors rather than silently ignored.
- Partial publication is recovered by a newly previewed and approved forward fix.

The clean-tree guardrail has no bypass, and hosted GitHub Releases are immutable create-or-verify: an existing Release is verified byte-identical to the intended one or the run fails with a conflict, never edited in place.
