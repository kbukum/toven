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
remote = "origin"
branches = ["main"]
registry = "crates-io"
offline = false
token_env = "CARGO_REGISTRY_TOKEN"
readiness = ["clean-tree", "registry-idempotent"]

[ecosystems.rust.release.prerelease]
channels = ["alpha", "beta", "rc"]
branch_channels = { next = "beta" }

[ecosystems.rust.release.changelog]
path = "CHANGELOG.md"
required = true

[ecosystems.rust.release.sign]
enabled = true

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
| `remote` | Git remote for release pushes | `origin` |
| `branches` | Allowed release branches; an empty list permits any branch | Any branch |
| `registry` | Intended registry selection | No registry |
| `offline` | Use tags rather than target queries for idempotency | `false` |
| `token_env` | Name of the registry-token environment variable, never the token | None |
| `readiness` | Named fail-closed checks | None |

Templates accept the documented release variables such as `{ecosystem}`, `{module}`, and `{version}`. Unknown placeholders, blank names, unsafe paths, and unsupported readiness checks fail validation.

A repository creates one release commit for all selected modules, so `commit_message` must render identically for every module in that repository. Module- or version-specific commit templates are suitable only when the repository releases one module at a time.

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

An undeclared channel is rejected. The channel becomes part of the semantic version and therefore part of the Rust or Go tag.

## Changelogs

```toml
[ecosystems.rust.release.changelog]
path = "CHANGELOG.md"
required = true
```

The contract requires a maintainer-readable entry for every directly changed release unit and a dependency-cascade explanation for every indirectly selected unit. The current planner produces deterministic changed-path summaries and rejects a required directly changed module when it has no change records. It does not yet parse, update, or verify an entry in the configured changelog file; file-backed changelog enforcement remains release-correctness work.

## Readiness

```toml
[ecosystems.rust.release]
readiness = ["clean-tree", "registry-idempotent"]
```

`clean-tree` and `registry-idempotent` are currently executable. A protected publication policy should configure both for registry releases and at least `clean-tree` for tag-only releases.

## Hosted GitHub Releases

```toml
[ecosystems.rust.release.host]
forge = "github"
draft = false
assets = ["dist/core.tar.gz", "dist/SHA256SUMS"]
```

Asset paths are project-relative exact paths; glob expansion is not implemented. An unset `prerelease` flag derives from the selected version. An unset `notes` value uses the planner's changelog summary.

Hosted Release rehearsal is mutation-free. Real hosted Release creation currently runs only after a pushing `release publish`, not after `release tag`.

## Registry and tag-only policy

The contract distinguishes:

| Policy | Rust | Go |
|---|---|---|
| Registry | Publish the crate, then verify the immutable registry version | Not applicable |
| Tag-only | Create the immutable tag without registry publication | The tag is the release |
| Excluded module | Do not version, tag, publish, or host | Do not version or tag |

The typed configuration currently accepts `registry`, but publication does not yet branch on it. The planned per-module `publish = true|false` field is not accepted yet. Do not add that field to a repository configuration until the release-correctness step implements it.

For Go test-only and benchmark modules, the policy is intentionally explicit: each such module must eventually set `release.publish = true` or `false`; path and name heuristics are forbidden.

## Signing

```toml
[ecosystems.rust.release.sign]
enabled = true
```

The typed configuration validates signing intent and an optional non-secret signer identifier. Artifact signing is not yet executed by `toven release`. Toven's own distribution contract uses keyless Sigstore/cosign signing in GitHub Actions with OIDC; no private key is stored in Toven configuration.

## Safety

- Preview commands must not mutate manifests, commits, tags, registries, or hosted Releases.
- Real publication requires `--yes`, an allowed branch, and a clean tree.
- Release tags, registry versions, hosted Releases, and hosted assets are immutable.
- Tokens remain in environment variables or credential stores.
- Partial publication is recovered by a newly previewed and approved forward fix.

The current `--allow-dirty` bypass and mutable GitHub Release reconciliation do not satisfy the final contract and must not be used in protected release automation.
