# Release modules

`toven release <action>` plans, inspects, rehearses, and applies dependency-aware releases across a repository.

## Maintainer journey

Use the release lifecycle in this order:

1. Run `toven release plan` and review every selected module, proposed version, reason, cascade origin, changelog summary, publication policy, and publication order.
2. Run `toven release status` to compare publication policy, declared versions, release tags, and versions reported by the ecosystem target.
3. Run `toven release readiness` and stop unless every configured check passes.
4. Generate review evidence with `toven release sbom` and `toven release depgraphs`.
5. Run `toven release publish --dry-run` and preserve `--output jsonl` output in CI.
6. Obtain human approval against that exact preview.
7. Run one mutating command with `--yes` from a clean, protected release branch.
8. Verify every expected tag, registry version, hosted asset, checksum, signature, SBOM, and provenance record.

Planning, status, readiness, dependency graphs, and publication rehearsal do not modify manifests, commits, tags, registries, or hosted Releases. `sbom` writes local artifacts and may invoke ecosystem tooling. Only `tag` and non-dry-run `publish` enter the mutation pipeline.

```bash
toven release plan
toven release status
toven release readiness
toven release sbom --out-dir target/toven/release/sbom
toven release depgraphs --out-dir target/toven/release/depgraphs
toven release publish --dry-run --output jsonl
toven release publish --yes
```

`toven release` without an action is a usage error.

## Actions

| Action | Result | Mutation |
|---|---|---|
| `plan` | Selected modules, publication policies, independent versions, tags, reasons, cascades, and order | None |
| `status` | Publication policies, declared versions, matching tags, and reported published versions | None |
| `readiness` | Fail-closed go/no-go checks | None |
| `sbom` | CycloneDX artifacts under `--out-dir` for supported targets | Local artifacts |
| `depgraphs` | DOT dependency graphs under `--out-dir` | Local artifacts |
| `package` | Archive an already-built binary into its declared per-target `host.assets` archive under `dist/` | Local artifacts |
| `checksums` | Write `SHA256SUMS` over every declared archive and the SBOM | Local artifacts |
| `sign` | Keyless Sigstore/cosign signature and certificate over `SHA256SUMS` | Local artifacts |
| `verify` | Presence/version-check local assets, or with `--download` verify the signature and every published archive's checksum | None |
| `tag` | Manifest changes, release commit, tags, and configured push | Repository and remote |
| `publish --dry-run` | Registry and hosted-Release rehearsal | None |
| `publish` | `tag` behavior followed by target publication and configured hosted Releases | Repository, remote, target, and forge |

Read-only tables and JSONL records use stdout. Warnings, mutating progress, summaries, and errors use stderr.

## Plan and status

```text
toven release plan [--output human|jsonl]
toven release status [--output human|jsonl]
```

The plan is deterministic and follows dependency order; repeated runs over unchanged state produce identical output. Each entry reports the current and planned version, the exact release tag a mutating run would create, the bump level, whether the module changed directly or joined through a dependency cascade, the winning version input, the resolved publication policy, and whether registry publication is needed. JSONL additionally carries the 1-based publication `order`, the cascade origin, prerelease channel, publication policy, and registry identifier when one exists. Entries appear in publication order in both renderings.

### Release baseline

Change detection answers "what changed since the last release", so the implicit baseline is the module's own latest release tag — never a branch ref. `[project].base_ref` and `[[members]].base_ref` select the baseline for changed-selection commands such as `toven affected`, not for releases; use `--base <REF>` to override a release diff explicitly.

A module with no release tag has never been released, so it always joins the plan as an initial release with reason `initial-release`. A first release cuts the version the module already declares — `0.1.0-alpha.1` is tagged as `0.1.0-alpha.1` — instead of bumping past it, because bumping would publish a version nobody declared and leave the declared one permanently unreleased. Explicit argv (`--patch`/`--minor`/`--major`, `--set-version`, `--pre`) still wins when a deliberate first bump is wanted.

Status performs read-only tag and ecosystem-target lookups and reports the resolved publication policy for each releasable module. A lookup failure is surfaced rather than converted into a successful empty result. With `offline = true`, status anchors on release tags and skips registry lookups entirely, so the projection stays network-free.

### Release notes

A hosted Release's body is generated from git, not from a `CHANGELOG.md` file: `toven` reads each module's commit range (`baseline..HEAD`, scoped to the module's own directory), classifies every commit as a [Conventional Commit](https://www.conventionalcommits.org/), and renders grouped, attributed bullets under Keep a Changelog headings — `### Breaking changes`, `### Added`, `### Fixed`, `### Changed`, `### Other`. This is forge-agnostic and deterministic: the same commits drive a GitHub or GitLab release body identically, with no forge API call or hand-maintained changelog to drift.

Each bullet carries the commit's optional scope, description, author attribution, and short id: `- **scope**: description — by @handle (abc123def456)`. The `@handle` is derived from git alone — a `login@users.noreply.github.com` or `ID+login@users.noreply.github.com` author email yields `@login`, and `Co-authored-by:` trailers are honored — falling back to the git author name when no handle is derivable (a commit authored with a personal email cannot be mapped to a forge handle without a network lookup, which `toven` deliberately avoids). Breaking changes are surfaced as a `### Breaking changes` section (from a `type!:` marker or a `BREAKING CHANGE:` body trailer) but do not silently re-decide the version bump — the bump stays driven by explicit `--minor`/`--major` argv or per-module config.

When a single-version workspace maps every module onto one hosted Release (a `v{version}` tag format), the per-module note bodies are merged into one: sections are unioned by heading and duplicate bullets dropped, so a `### Added` heading appears once rather than once per contributing crate. A module with no commits in range contributes an empty body (the plan table's `dependency cascade` / `initial release` summary is never emitted as release prose) and folds away against a sibling that does carry notes.

The rendered body is fully previewable mutation-free through `toven release publish --dry-run` (see below) before any release is cut.

## Readiness

```bash
toven release readiness
```

Recognized checks are:

| Check | Meaning |
|---|---|
| `clean-tree` | Every member repository has no uncommitted changes |
| `registry-idempotent` | No registry-published module declares a version lower than the highest version reported by its release target |

Any failed check returns a non-zero exit status. An unknown check is invalid configuration. Readiness is evidence for approval; the mutating pipeline independently enforces its clean-tree guard.

## SBOM and dependency graphs

```bash
toven release sbom --out-dir dist/sbom
toven release depgraphs --out-dir dist/graphs
```

Artifact paths are written to stdout. Unsupported-ecosystem skips are warnings on stderr. Rust SBOM generation currently requires `cargo-cyclonedx`; Go SBOM generation is not implemented.

## Binary release artifacts

For a binary-distributed workspace, the fixed `host.assets` set is assembled by four non-mutating verbs — the same ones `.github/workflows/release.yml` drives, each writing into the local `dist/` directory rather than touching a tag, registry, or hosted Release:

```bash
toven release package --target x86_64-unknown-linux-gnu   # per built target
toven release checksums                                    # SHA256SUMS over archives + SBOM
toven release sign                                         # keyless Sigstore signature over SHA256SUMS
toven release verify --no-run                              # presence-check the declared asset set
```

`package` archives an already-built binary for `--target` into the exact declared per-target archive path (globbing and version placeholders are not supported — the `host.assets` list is a set of fixed project-relative paths). `checksums` writes a SHA-256 `SHA256SUMS` covering every declared archive and the SBOM. `sign` produces the keyless Sigstore/cosign signature and certificate over `SHA256SUMS`; it runs only when `[ecosystems.<id>.release.sign] enabled = true` and matches the configured keyless `identity`/`issuer`. `verify` presence- and version-checks the local asset set; with `--download` it fetches every published asset, verifies the Sigstore signature on `SHA256SUMS` first, then checksum-verifies each archive before extraction. `--no-run` skips executing the archived binaries, so the whole multi-target asset set can be verified from a single runner that cannot execute every target.

## Mutation-free publication rehearsal

```bash
toven release publish --dry-run
toven release publish --dry-run --output jsonl > release-preview.jsonl
```

The rehearsal resolves the same module order, versions, publication policies, target idempotency verdicts, hosted tags, prerelease flags, and configured asset paths as a real publish. Registry entries report `would-publish` or `already-published`; tag-only entries report `tag-only`. Each hosted Release it would cut is previewed with its fully rendered, commit-derived notes body (human output prints the body under the hosted-release table; JSONL carries it as a `notes` field), so the exact release prose can be reviewed before dispatch. It does not call manifest mutation, packaging, publication, tag creation, push, or forge commands.

Version choices can be supplied to rehearsal and mutating actions:

```bash
toven release publish --dry-run --minor rust:core
toven release publish --dry-run --set-version rust:cli=2.0.0
toven release publish --dry-run --pre rc --base origin/main
```

| Option | Meaning |
|---|---|
| `--patch <MODULE>` | Force a patch bump; repeatable |
| `--minor <MODULE>` | Force a minor bump; repeatable |
| `--major <MODULE>` | Force a major bump; repeatable |
| `--set-version <MODULE>=<VERSION>` | Set an exact version; repeatable |
| `--pre <CHANNEL>` | Select a configured prerelease channel |
| `--base <REF>` | Override the change baseline |
| `--offline` | Skip target version queries and use release tags for idempotency |

Conflicting overrides fail before mutation.

## Approval and clean-tree enforcement

```bash
toven release tag --yes
toven release publish --yes
```

Mutating actions fail unless `--yes` is present. They check the allowed branch and reject a dirty worktree before changing a manifest — the clean-tree guardrail has no bypass. Before planning any version bump, a **reconcile pre-pass** completes a hosted Release for the version that is already published. A publish that pushed a module's tag and published its registry version but stopped before cutting the forge Release leaves an immutable, un-hostable state the normal bump planner can never reach — a changed module always plans a forward bump onto a fresh tag, and an unchanged module drops out of the plan entirely. Keyed on that already-published-and-tagged-but-unhosted state, re-running `publish` detects the current published version whose tag exists but whose Release is missing and creates only that Release through the forge's create-or-verify path, then exits without a bump, commit, tag, push, or re-publish. It runs only for a pushing publish (the hosted Release needs the pushed tag) and short-circuits the run only when it actually creates a missing Release; when every candidate Release already exists it creates nothing and falls through to a normal release, so a legitimate new version is never blocked. An existing Release is probed read-only and left untouched — the immutable verify never runs here, so a Release whose notes legitimately differ from freshly authored ones is not reported as a conflict on re-dispatch. The run is marked resumed and the operator sees a reconcile notice.

Before any mutation, the planned tags are preflighted and classified against the tags that already exist. When none of the planned tags exist, the run proceeds normally. When every planned tag already exists and the intra-plan annotations agree, the run **resumes**: it skips the prepare, commit, tag, and push steps entirely and lets the idempotent registry-publish and hosted-Release phases finish any remaining work — this completes a run that pushed its tags and published its registry versions but stopped before creating the hosted Release when the versions are pinned to those already released (for example a `--set-version` recovery), without touching the immutable tag or registry. The run is marked resumed and the operator sees a resume notice. A partial or divergent planned-tag set — some tags present and others missing — fails closed with forward-fix guidance, because tags are immutable and a partially-tagged release cannot be safely re-derived. `--no-push` keeps the release commit and tags local and therefore skips both the reconcile pre-pass and hosted Release creation. When the release branch is protected, `push_branch = false` pushes only the tags and leaves the branch to the pull-request flow. A failure after the release commit — tagging, push, registry publication, or hosted Release creation — reports the externally visible state and the forward-only recovery path; nothing is rolled back past that boundary.

The tag/branch push authenticates over HTTPS using a token read from the variables listed in [`[toven.git].push_token_env`](../config/README.md#runtime) (default `GITHUB_TOKEN`, then `GH_TOKEN`). In CI the workflow exposes the job token under one of those names; locally, with none set, the push falls back to the ambient git transport default.

## Rust release policy

The supported Rust contract is:

- Cargo packages receive independent semantic versions.
- Changed crates use the configured or per-run bump.
- Dependency requirement changes cascade into dependents according to `dependent_version`.
- Registry-enabled crates publish in dependency order when `registry = "crates-io"` is configured.
- Tag-only crates stop after immutable Git tags and may still produce hosted assets.
- Stable and configured prerelease channels use normal semantic-version precedence.
- Required changelog evidence and readiness checks fail before mutation.

The current implementation supports independent versions, Cargo manifest mutation, dependency-floor cascades, prereleases, deterministic order, crates.io publication for registry-enabled crates, and tag-only Rust releases by default. `release tag` stops before target publication and hosted Release creation.

## Go release policy

The supported Go contract is:

- Only changed modules and required dependents join the release train.
- The root module uses `vX.Y.Z`.
- A nested module at `cache/redis` uses `cache/redis/vX.Y.Z`.
- Go module tags are fixed; `tag_format` is rejected.
- Prerelease versions use the same path prefix, for example `cache/redis/v1.2.0-alpha.1`.
- Dependency cascades that require `go.mod` requirement rewrites are rejected before mutation until the release mutation carries Go import paths; Toven does not rewrite requirements by module-name or path heuristics.
- Test-only and benchmark modules must declare tag-only release policy or `exclude = true`; Toven never infers policy from a module name or path.

The current implementation supports changed-module planning, dependency-graph cascades, reachable root and nested tag discovery, and prerelease tags. Go releases are tag-only; `registry` is rejected for Go. A module with no reachable release tag fails closed instead of using a synthetic `0.0.0` version, so the first Go release needs an explicit versioning path before mutation.

## Hosted assets and immutability

When `release.host.forge = "github"`, publication invokes `gh` after tags are pushed and target publication succeeds. Authentication comes from ambient `gh` configuration, `GH_TOKEN`, or `GITHUB_TOKEN`; secrets are not placed in argv.

The supported contract treats a published tag, registry version, hosted Release, and same-named asset as immutable. A retry may verify an identical completed result as already complete, but a conflicting tag, hosted Release, or same-named asset fails with forward-fix guidance. The GitHub adapter uses create-or-verify behavior and never edits an existing Release or uploads assets with clobber semantics.

## Recovery

Everything before a successful release commit is reversible and the implementation attempts to restore the worktree on failure. After a commit, tag, registry version, or hosted Release becomes externally visible, do not rewrite or delete it to make the run appear atomic. Inspect `release status`, correct the repository or release configuration, choose a new version where necessary, preview again, obtain approval again, and publish a forward fix.

Never force-move a release tag, overwrite a published package version, or replace an asset attached to an approved immutable Release.
