# Release configuration

Release policy is declared per ecosystem and optionally overridden per module. The policy controls selection, version decisions, tags, registry publication, readiness checks, prerelease channels, and hosted releases.

## Minimal Rust registry release

```toml
[ecosystems.rust.release]
registry = "crates-io"
readiness = ["clean-tree", "registry-idempotent"]

[ecosystems.rust.release.host]
forge = "github"
```

## Minimal Go tag release

```toml
[ecosystems.go.release]
readiness = ["clean-tree"]

[ecosystems.go.release.host]
forge = "github"
```

Go has no registry publication phase. The tag is the released version.

## Policy choices

| Choice | Behavior |
|---|---|
| Registry | Publish artifacts after tags are created |
| Tag-only | Record the release through immutable Git tags |
| Default bump | Select the initial bump for changed modules |
| Dependency cascade | Release dependents when dependency requirements change |
| Prerelease channels | Permit configured `alpha`, `beta`, or `rc` releases |
| Readiness | Require named go/no-go checks before mutation |
| Hosted release | Create or update a forge release after publication |
| Push policy | Permit or suppress commit and tag pushes |

## Per-module overrides

```toml
[modules."rust:internal-tool".release]
publish = false

[modules."rust:core".release]
level = "minor"
```

Use module overrides for exceptions. Keep ecosystem defaults consistent for the common case.

## Prereleases

```toml
[ecosystems.rust.release.prerelease]
channels = ["alpha", "beta", "rc"]
```

Select a configured channel at release time:

```bash
toven release publish --pre rc --yes
```

An unconfigured channel is rejected.

## Readiness

```toml
[ecosystems.rust.release]
readiness = ["clean-tree", "registry-idempotent"]
```

- `clean-tree` requires no uncommitted changes.
- `registry-idempotent` rejects declared versions behind registry state.

Unknown check names fail configuration validation.

## Hosted GitHub Releases

```toml
[ecosystems.rust.release.host]
forge = "github"
assets = ["dist/*.tar.gz", "dist/SHA256SUMS"]
```

The hosted release runs after the tag is pushed and registry publication succeeds. Assets are resolved from the project root. GitHub authentication comes from the ambient `gh` configuration or `GH_TOKEN`/`GITHUB_TOKEN`.

## Rust tag policy

Rust targets default to a target-owned tag scheme that identifies the ecosystem, module, and version. Registry-enabled crates publish in dependency order. Tag-only crates stop after the Git release phase.

## Go tag policy

Go tags follow module conventions:

```text
v1.2.3
cache/redis/v1.2.3
```

The root module uses `v<version>`. A nested module prefixes the version with its repository-relative module root. Custom Go `tag_format` values are rejected.

## Precedence

Version decisions resolve from highest to lowest priority:

```text
release CLI override > module release override > ecosystem release policy > adapter default
```

`toven release plan` reports the winning input for each module.

## Safety

- Preview commands do not mutate source trees, tags, registries, or forges.
- `release tag` and `release publish` require `--yes`.
- Published versions and pushed tags are immutable.
- Secrets must remain in environment variables or tool credential stores.
- Partial releases are recovered through a new forward-fix version.

Run the complete [release workflow](../commands/release.md) before approval.
