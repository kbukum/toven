# Release modules

Start with a read-only release plan:

```bash
toven release plan
```

`toven release <action>` plans, inspects, rehearses, and applies dependency-aware releases across a repository. `toven release` without an action is a usage error.

## Maintainer path

Run the release lifecycle in this order:

1. Run `toven release plan` and review the selected modules, proposed versions, reasons, cascade origins, changelog summaries, publication policies, and publication order.
2. Run `toven release status` to compare publication policy, declared versions, release tags, and versions reported by the ecosystem target.
3. Run `toven release readiness` and stop unless every configured check passes.
4. Generate review evidence with `toven release sbom` and `toven release depgraphs`.
5. Run `toven release publish --dry-run` and preserve `--output jsonl` output in CI.
6. Obtain human approval against that exact preview.
7. Run one mutating command with `--yes` from a clean, protected release branch.
8. Verify every expected tag, registry version, hosted asset, checksum, signature, SBOM, and provenance record.

```bash
toven release plan
toven release status
toven release readiness
toven release sbom --out-dir target/toven/release/sbom
toven release depgraphs --out-dir target/toven/release/depgraphs
toven release publish --dry-run --output jsonl
toven release publish --yes
```

Planning, status, readiness, dependency graphs, and publication rehearsal do not modify manifests, commits, tags, registries, or hosted Releases. `sbom` writes local artifacts and may invoke ecosystem tooling. Only `tag` and non-dry-run `publish` enter the version-cut mutation pipeline.

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
| `image` | Build the configured container image once, push it to the primary registry plus mirrors, and cosign-sign the pushed digest | Registries |
| `provenance` | Verify SLSA provenance exists over exactly the published subjects: declared `SHA256SUMS` manifest entries and every pushed image digest (the CI trusted builder creates it) | None (read-only) |
| `bump` | Manifest version/floor changes and the rolled changelog, committed or staged with `--no-commit` | Repository working tree |
| `tag` | Manifest changes, release commit, tags, and configured push | Repository and remote |
| `publish --dry-run` | Registry and hosted-Release rehearsal | None |
| `publish` | `tag` behavior followed by target publication and configured hosted Releases | Repository, remote, target, and forge |

Read-only tables and JSONL records use stdout. Warnings, mutating progress, summaries, and errors use stderr.

## Plan and status

```text
toven release plan [--output human|jsonl]
toven release status [--output human|jsonl]
```

The plan is deterministic and follows dependency order. Repeated runs over unchanged state produce identical output.

Each plan entry reports the current and planned version, exact release tag, bump level, direct-change or dependency-cascade reason, winning version input, release flow, publication policy, and whether registry publication is needed. The tag is either the tag a Toven-owned run would create or, for a maintainer-owned module, the existing tag Toven verifies. The release flow includes its `entrypoint`; aggregate modules carry an `umbrella` marker.

JSONL also carries the 1-based publication `order`, cascade origin, prerelease channel, publication policy, registry identifier when one exists, `entrypoint`, and `umbrella` flag. Human and JSONL output both appear in publication order.

`release status` reports each module's flow and `entrypoint`. For maintainer-owned modules, it reports whether the required release tag for the declared version exists. Human output uses `tag ready` or `tag missing`; JSONL uses `maintainer_tag_present`. A maintainer-owned module cannot publish until its tag exists. See [entrypoint flows](../config/release.md#entrypoint-flows-toven-owned-and-maintainer-owned).

## Release baseline

Release change detection asks what changed since the module's latest release tag. It does not use a branch ref by default. `[project].base_ref` and `[[members]].base_ref` apply to changed-selection commands such as `toven affected`.

Use `--base <REF>` to override a release diff explicitly. A module with no release tag always joins the plan as an initial release with reason `initial-release`. Its first release cuts the version the module already declares, such as `0.1.0-alpha.1`, instead of bumping past it.

Explicit version argv still wins when you want a deliberate first bump: `--patch`, `--minor`, `--major`, `--set-version`, or `--pre`.

Status performs read-only tag and ecosystem-target lookups and reports each releasable module's publication policy. Lookup failures surface as errors. With `offline = true`, status uses release tags for idempotency and skips registry lookups.

Status also reports hosted forge participation. The `Hosted on` column, or `host_forge` under `--output jsonl`, names the forge resolved from a `[…release.host]` block. It stays blank when no host forge resolves. Host participation is separate from publication policy, so a registry library can still contribute notes to a shared hosted Release.

## Release notes

Hosted Release bodies come from git. Toven reads each module's commit range, `baseline..HEAD`, scoped to the module directory. It classifies Conventional Commits and renders grouped bullets under Keep a Changelog headings: `### Breaking changes`, `### Added`, `### Fixed`, `### Changed`, and `### Other`.

The notes are forge-agnostic and deterministic. A GitHub or GitLab release body comes from the same commits, without a forge API call or hand-maintained changelog.

Each bullet carries optional scope, description, author attribution, and short id:

```text
- **scope**: description — by @handle (abc123def456)
```

The `@handle` is derived from git only. `login@users.noreply.github.com` and `ID+login@users.noreply.github.com` author emails become `@login`, and `Co-authored-by:` trailers are honored. Toven falls back to the git author name when no handle can be derived.

Breaking changes come from a `type!:` marker or a `BREAKING CHANGE:` body trailer. They add a `### Breaking changes` section. They do not decide the version bump; explicit `--minor` or `--major` argv, or per-module config, still controls the bump.

When a single-version workspace maps every module onto one hosted Release with a `v{version}` tag format, Toven merges per-module note bodies. Sections are unioned by heading and duplicate bullets are dropped. A module with no commits in range contributes an empty body and folds away against a sibling that carries notes.

The plan table's summary column is only a table cell. It shows a commit count such as `1 commit` or `3 commits`, or `dependency cascade` / `initial release` when no commits are in range. It is never emitted as release-body prose.

Preview the rendered body before cutting anything:

```bash
toven release publish --dry-run
```

## Readiness

```bash
toven release readiness
```

Recognized checks:

| Check | Meaning |
|---|---|
| `clean-tree` | Every member repository has no uncommitted changes |
| `registry-idempotent` | No registry-published module declares a version lower than the highest version reported by its release target |

Any failed check returns a non-zero exit status. An unknown check is invalid configuration. Readiness is evidence for approval; the mutating pipeline also enforces its own clean-tree guard.

## SBOM and dependency graphs

```bash
toven release sbom --out-dir dist/sbom
toven release depgraphs --out-dir dist/graphs
```

Artifact paths are written to stdout. Unsupported-ecosystem skips are warnings on stderr. Rust SBOM generation requires `cargo-cyclonedx`. Go SBOM generation requires `cyclonedx-gomod` and produces one CycloneDX `<module>.cdx.json` per module.

## Binary release artifacts

Binary-producing modules declare `host.assets`. Four non-mutating verbs assemble those assets into the local `dist/` directory. The release workflow drives the same four verbs.

```bash
toven release package --target x86_64-unknown-linux-gnu
toven release checksums
toven release sign
toven release verify --no-run
```

These verbs scope to modules that declare assets. Registry libraries in a mixed repository carry no archives, so only the binary app is packaged, checksummed, signed, and verified.

`package` archives an already-built binary for `--target` into the exact declared per-target archive path. It does not support globbing or version placeholders; `host.assets` is a set of fixed project-relative paths. Use `--binary <PATH>` to package an explicit binary path.

`checksums` writes a SHA-256 `SHA256SUMS` covering every declared archive and the SBOM. `sign` creates the keyless Sigstore/cosign signature and certificate over `SHA256SUMS`. It runs only when `[ecosystems.<id>.release.sign] enabled = true` and matches the configured keyless `identity` and `issuer`.

`verify` presence- and version-checks the local asset set. With `--download`, it fetches every published asset, verifies the Sigstore signature on `SHA256SUMS`, then checksum-verifies each archive before extraction. `--no-run` skips executing archived binaries, so one runner can verify a multi-target asset set.

## Delegated artifact phases

`package` and `sign` can be backed by an external tool instead of Toven's native archiver or signer. That lets an established ecosystem workflow plug into the Toven-owned flow unchanged.

The canonical example is [GoReleaser](https://goreleaser.com). A Go binary module sets `[…release.phases.package] backing = "delegated"` and a `[…release.phases.package.delegated]` tool block. See [phase-backing config](../config/release.md#release-phases-and-backing).

During `package`, Toven runs the tool's mutation-free preview, such as GoReleaser's `--snapshot`, then normalizes produced archives at the declared `host.assets` paths back into typed JSONL. Each reported asset carries `backing = "native"` or `backing = "delegated"`.

Toven still owns selection, ordering, tag creation, readiness, the mutation-free preview guarantee, and the single shared hosted Release. Only artifact production is delegated. A delegated backing that cannot preview mutation-free is rejected.

Flow-ownership phases cannot be delegated: `select`, `bump`, `tag`, `publish`, and `host`. Toven also rejects `backing = "delegated"` for `image` and `provenance` at plan time. Keep those phases native.

The phase model treats `image` and `provenance` as delegable in principle, but Toven rejects delegated execution for them. Because the flow is language-agnostic, a Go binary module can attach archives natively or through GoReleaser while sibling library modules tag in lock-step. They can all feed one `v{version}` hosted Release.

## Container images and provenance

A service-style module declares a `[…release.image]` block instead of shipping as a registry package or archive. See [container image release config](../config/release.md#container-image-release). Two native verbs complete the release: `image` publishes the container image, and `provenance` verifies SLSA build provenance over what was published.

`image` previews mutation-free with `--dry-run`; a real `image` writes to registries, so it requires `--yes`. `provenance` is read-only — it does not create attestations (the CI trusted builder does that with `actions/attest-build-provenance`), it verifies they exist — so it needs no `--yes`; its `--dry-run` reports subject presence without failing.

```bash
toven release image --dry-run
toven release image --yes
toven release provenance --dry-run
toven release provenance
```

`image` runs only for modules that declare an image block. A module without one is skipped. A run where no module declares an image block fails closed.

For each image module, Toven renders the image name and tag from the declared version, builds the context and Dockerfile once, pushes the digest to the primary registry, then pushes every configured mirror. When `sign = true`, which is the default, Toven cosign-signs the pushed digest keyless.

Image publication is immutable. Pushing a tag that already exists at a different digest fails closed. Recovery is a forward-fix version, never a moved tag. An already-present identical digest reports `already-complete`.

Registry credentials come from the ambient environment only. They are never placed on argv or logged. `--dry-run` resolves each reference's existing digest but never builds, pushes, or signs. It reports `would-push` or `already-present`.

`provenance` verifies exactly the approved, published subjects. Subjects are the entries of the declared `SHA256SUMS` manifest, each with a `sha256:` digest and located by its bare project-relative path beside the manifest, plus the live digest of every pushed image reference. Toven shells to `gh attestation verify` (argv-only) — a file subject by its path, an image subject by its digest-pinned `oci://name@sha256:…` reference — against the repository slug it resolves from the working directory, and binds the check to the trusted builder with `--signer-workflow .github/workflows/release.yml`, so an attestation cut by any other workflow in the same repository does not satisfy it. Before verifying a file subject, Toven hashes its on-disk bytes and fails closed if they do not match the manifest digest it reported. Manifest entries are validated at this trust boundary: a name with a path separator or `..` is rejected rather than allowed to escape the release directory.

`provenance` fails closed when neither a `SHA256SUMS` manifest nor an image is declared, when a declared manifest lists no subjects, when a declared image resolves to no pushed digest, or — outside `--dry-run` — when any published subject lacks an attestation. Run `toven release image` first for image subjects.

Verification is read-only and idempotent. A default run reports `verified` once every subject carries an attestation. The forge token comes from the ambient environment only. A missing attestation is recognized only by `gh`'s explicit "no attestations found" result; any other tool failure (auth, an inaccessible private repository, network) fails closed rather than being read as absent.

`--dry-run` reports, per subject, whether an attestation exists as `present` or `missing` and never fails on a missing one — an attested subject is never masked by an unattested sibling. Typed JSONL emits one record per module image for `image` or per subject for `provenance`, with `preview` and the subject's resolved `status`; data goes to stdout and warnings go to stderr.

## Mutation-free publication rehearsal

```bash
toven release publish --dry-run
toven release publish --dry-run --output jsonl > release-preview.jsonl
```

The rehearsal resolves the same module order, versions, publication policies, target idempotency verdicts, hosted tags, prerelease flags, and configured asset paths as a real publish. Registry entries report `would-publish` or `already-published`; tag-only entries report `tag-only`.

Each hosted Release preview includes the fully rendered, commit-derived notes body. Human output prints the body under the hosted-release table. JSONL carries it as a `notes` field. The rehearsal does not call manifest mutation, packaging, publication, tag creation, push, or forge commands.

Supply version choices to rehearsal and mutating actions:

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

## Bump versions and changelogs

```bash
toven release bump --yes
toven release bump --no-commit --yes
toven release bump --dry-run
toven release bump --output jsonl
```

`bump` runs only the version decision phase. It rewrites each selected module's manifest version and dependency floors. Where configured, it also rolls the changelog.

By default, `bump` creates the release commit. With `--no-commit`, it leaves the mutation staged for a maintainer pull request. It never tags, pushes, publishes, or cuts a hosted Release. In the Toven and rskit release flow, the version and changelog change is the release decision; tag and publish follow after it merges.

`bump`, `tag`, and `publish` share the same manifest-mutation prefix through the `ManifestMutator` phase contract. The versions a `bump` commit carries are exactly the versions a later `tag` would produce. The same version-input flags apply: `--patch`, `--minor`, `--major`, `--set-version`, `--pre`, and `--base`.

A mutating `bump` requires `--yes`. It checks the allowed branch and a clean worktree before mutation. `--dry-run` previews planned version transitions and changelog paths without writing, so it does not need `--yes`. `--no-commit` is valid only on `bump`; `--no-push` is not, because `bump` never pushes.

| Option | Meaning |
|---|---|
| `--no-commit` | Stage the manifest/changelog mutation for a pull request instead of committing |
| `--dry-run` | Preview the version transitions and changelog roll without writing |

Typed JSONL emits one record per bumped module. Each record carries `module`, `old_version`, `new_version`, rewritten `manifests`, whether the run `committed`, and rolled `changelogs`. Data goes to stdout and warnings go to stderr.

### Changelog rolling

`bump` verifies the changelog the same way `tag` does. When a module's release config sets `[release.changelog].roll = true`, it moves the documented `## [Unreleased]` body into `## [x.y.z] - <date>` and leaves an empty `[Unreleased]` at the top.

Rolling only moves maintainer-written prose. It never fabricates release notes. Rolling is opt-in; with `roll` unset, the default, the changelog is verified but left byte-for-byte unchanged.

If a repository both rolls in `bump` and requires a documented `[Unreleased]` in `tag`, run `tag` against the merged, rolled changelog. That is the pull-request-first flow.

## Approval and clean-tree checks

```bash
toven release tag --yes
toven release publish --yes
```

Mutating actions fail unless `--yes` is present. They check the allowed branch and reject a dirty worktree before changing a manifest. The clean-tree guardrail has no bypass.

Before planning a version bump, a reconcile pre-pass completes a hosted Release for a version that is already published. If a publish pushed a module tag and published the registry version but stopped before the forge Release, re-running `publish` detects that already-published-and-tagged-but-unhosted state. It creates only the missing Release through the forge's create-or-verify path, then exits without a bump, commit, tag, push, or re-publish.

The reconcile path runs only for a pushing publish because the hosted Release needs the pushed tag. It short-circuits only when it creates a missing Release. If every candidate Release already exists, it creates nothing and continues to normal release planning. Existing Releases are probed read-only and left untouched. The operator sees a reconcile notice.

Before mutation, Toven preflights planned tags against existing tags. When no planned tags exist, the run proceeds. When every planned tag exists and intra-plan annotations agree, the run resumes: it skips prepare, commit, tag, and push, then lets idempotent registry-publish and hosted-Release phases finish. This completes a run that already pushed tags and published registry versions, including a `--set-version` recovery. The operator sees a resume notice.

A partial or divergent planned-tag set fails closed. That means some tags exist and others do not, or annotations disagree. Because tags are immutable, a partially tagged release cannot be safely re-derived.

`--no-push` keeps the release commit and tags local. It skips both the reconcile pre-pass and hosted Release creation. When the release branch is protected, `push_branch = false` pushes only tags and leaves the branch ref untouched.

A failure after the release commit reports externally visible state and a forward-only recovery path. Nothing is rolled back past that boundary.

Tag and branch pushes authenticate over HTTPS using a token from the variables listed in [`[toven.git].push_token_env`](../config/README.md#runtime). The default order is `GITHUB_TOKEN`, then `GH_TOKEN`. In CI, expose the job token under one of those names. Locally, with none set, push falls back to the ambient git transport default.

When `sign_tags = true`, `tag` and `publish` create cryptographically signed annotated tags. Signing is always annotated, so `tag_message` is required and signed lightweight tags are rejected at validation.

`sign_format` selects the backend: `openpgp`/`gpg`, `ssh`, or `x509`. It maps to git's `gpg.format`. `signing_key` pins the key identifier and maps to git's `user.signingkey`. Both are optional and inherit repository git configuration when unset. The signing key identifier is never key material. If signing is requested and no key resolves, the run fails closed before creating the tag.

## Rust release policy

Supported Rust behavior:

- Cargo packages receive independent semantic versions.
- Changed crates use the configured or per-run bump.
- Dependency requirement changes cascade into dependents according to `dependent_version`.
- Registry-enabled crates publish in dependency order when `registry = "crates-io"` is configured.
- Tag-only crates stop after immutable Git tags and may still produce hosted assets.
- Stable and configured prerelease channels use normal semantic-version precedence.
- Required changelog evidence and readiness checks fail before mutation.

Rust release support includes independent versions, Cargo manifest mutation, dependency-floor cascades, prereleases, deterministic order, crates.io publication for registry-enabled crates, and tag-only Rust releases by default. `release tag` stops before target publication and hosted Release creation.

## Go release policy

Supported Go behavior:

- Only changed modules and required dependents join the release train.
- The root module uses `vX.Y.Z`.
- A nested module at `cache/redis` uses `cache/redis/vX.Y.Z`.
- Go module tags are fixed; `tag_format` is rejected.
- Prerelease versions use the same path prefix, for example `cache/redis/v1.2.0-alpha.1`.
- Dependency cascades that require `go.mod` requirement rewrites are rejected before mutation, because Toven does not carry Go import paths for safe rewrites and does not rewrite requirements by module-name or path heuristics.
- Test-only and benchmark modules must declare tag-only release policy or `exclude = true`; Toven never infers policy from a module name or path.

Go release support includes changed-module planning, dependency-graph cascades, reachable root and nested tag discovery, and prerelease tags. Go releases are tag-only; `registry` is rejected for Go. A module with no reachable release tag fails closed instead of using a synthetic `0.0.0` version, so the first Go release needs an explicit versioning path before mutation.

## Hosted assets and immutability

When `release.host.forge = "github"`, publication invokes `gh` after tags are pushed and target publication succeeds. Authentication comes from ambient `gh` configuration, `GH_TOKEN`, or `GITHUB_TOKEN`. Secrets are not placed in argv.

A published tag, registry version, hosted Release, and same-named asset are immutable. A retry may verify an identical completed result as already complete. A conflicting tag, hosted Release, or same-named asset fails with forward-fix guidance. The GitHub adapter uses create-or-verify behavior and never edits an existing Release or uploads assets with clobber semantics.

## Recovery

A publish has one hard rollback boundary. Manifest mutation, packaging, and the attempted release commit are reversible. A failure before the commit restores the worktree and creates no commit or tag.

The release commit, each module tag, the branch and tag push, registry publication, and hosted Release become externally visible in that order. Once visible, they are immutable. Recovery never rewinds past the commit boundary; it resumes forward.

### Diagnose first

Start every recovery with `release status`. It reports, per module, which versions are tagged, pushed, published to the registry, and hosted. Use it to locate where the interrupted run stopped before changing anything.

### Partial publication

Re-running `publish` after an interruption is safe. Mutation-free phases are idempotent, and mutating phases are guarded by tag preflight.

- **Interrupted before the commit**: mutation, packaging, or the commit failed. The worktree was restored and nothing is visible. Re-run normally; the plan is unchanged.
- **Interrupted after tags were pushed but before every registry version or hosted Release completed**: re-running resumes. Toven preflights the planned tags, finds them all present with agreeing annotations, skips prepare/commit/tag/push, and lets idempotent registry-publish and hosted-Release phases finish. A registry version that already exists reports `already-published`. A missing hosted Release is created by the reconcile pre-pass from the pushed tag. The run prints a resume or reconcile notice.
- **Interrupted with a partial or divergent planned-tag set**: some planned tags are present, others are missing, or annotations disagree. Toven fails closed with forward-fix guidance. Do not delete existing tags; publish a forward fix instead.

`--no-push` keeps the commit and tags local, so it has nothing externally visible to reconcile. Recover by discarding the local commit and tags and re-running.

### Forward-fix releases

When the repository or release configuration was wrong, correct the source and choose a new version where the released one is already visible. Examples include a bad manifest, wrong registry, or incorrect asset.

Preview with `release plan` and `release publish --dry-run`, obtain approval again, and publish the forward fix. A changed module always plans onto a fresh tag, so the forward fix does not collide with the immutable release it supersedes.

### Never do this

Never force-move or delete a release tag, overwrite a published registry version, or replace an asset attached to an approved immutable Release. These break the immutability contract downstream consumers rely on. The adapters refuse them: the GitHub adapter uses create-or-verify semantics and never clobbers an existing Release or asset.
