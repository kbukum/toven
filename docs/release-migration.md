# Release migration

Migrate a repository to Toven without replacing a working release path before parity is proven.

## Migration stages

1. Inventory the current release behavior.
2. Configure equivalent Toven release policy.
3. Run representative local fixtures.
4. Compare Toven previews against isolated copies of the real repository.
5. Publish the first Toven binary.
6. Run a direct-binary CI canary, pinned by version and checksum.
7. Remove competing release logic after parity.

The only current downstream install contract is a direct download of a released binary pinned by version and checksum. A `toven-action` that installs and runs Toven inside a workflow is a candidate future convenience wrapper around that same contract; it is explicitly deferred and is not a migration prerequisite.

## Inventory

Record:

- module selection rules
- version and prerelease rules
- dependency cascades
- tag grammar
- changelog requirements
- registry targets
- hosted assets
- signing, SBOM, and provenance
- approval and permissions
- partial-publication recovery

Every retained behavior must map to Toven configuration, a Toven command, or an explicitly native gate.

## Preview parity

Run:

```bash
toven release plan --output jsonl
toven release readiness --output jsonl
toven release sbom --out-dir target/toven/release/sbom
toven release depgraphs --out-dir target/toven/release/depgraphs
toven release publish --dry-run --output jsonl
```

Compare selected modules, target versions, cascade reasons, tags, order, prerelease classification, and hosted assets.

## Isolation

Use temporary clones or worktrees with local Git remotes and controlled registry or forge doubles. Preview verification must not mutate source repositories, manifests, tags, registries, or hosted releases.

## Bootstrap dependency

Downstream CI cannot install a released Toven binary until Toven has published one. Complete Toven's first binary release before requiring it in rskit or gokit.

## CI canary

The first CI integration should download a versioned binary directly and verify its checksum. Keep the existing release implementation available for comparison.

The direct, checksum-verified binary invocation is the canonical downstream contract; there is no required intermediary action to adopt.

## Cutover

Cut over only when:

- fixture and real-repository previews agree
- dry-runs are mutation-free
- safety failures are covered
- released binaries install and run in CI
- recovery procedures are documented

Remove obsolete scripts, tests, workflow branches, and documentation in the same change. Keep native developer commands only when they do not compete with release behavior.

## Recovery

Never rewrite published versions or pushed release tags. Inspect current state with:

```bash
toven release status
```

Correct the repository and publish a forward-fix version for incomplete or inconsistent release trains.

## Repository parity maps

Each self-hosted repository configures Toven against the executable release contract while its existing native release path stays authoritative until the `verify-real-repositories` check proves parity. The tables record, per repository, what Toven models, the native source of truth it must match, and which operations remain deliberately native.

### Toven

Toven is a single tag-only binary release train: every workspace crate is `publish = false` and resolves to the tag-only policy, and `tag_format = "v{version}"` collapses the shared workspace version onto one repository tag. Toven never publishes to crates.io.

| Concern | Toven models | Native source of truth | Parity required | Retained native |
|---|---|---|---|---|
| Module discovery | `toven modules` over `crates/*`, `apps/*` | Cargo workspace members | Yes | — |
| Version / tag | one `v{version}` tag, `release plan`/`status` | workspace `version`, git tags | Yes | — |
| Publication | tag-only, no registry | `publish = false` per crate | Yes | — |
| Hosted release | `host.forge = github` (assets attached at publish) | release-readiness workflow | Not yet (pending binary matrix build) | binary matrix build, signing, SBOM, provenance |
| Gate | fmt-check, lint, nextest, doc, deny via `make check` | Makefile | Partial | rustfmt, doctests, cargo-deny, ast-grep structure |

### rskit

rskit's publishable core and contrib crates publish to `crates-io`; the `examples/` demos and the `fuzz/` harness are `publish = false` and are explicitly `exclude`d from the release so workspace discovery never sweeps them into the registry train.

| Concern | Toven models | Native source of truth | Parity required | Retained native |
|---|---|---|---|---|
| Module discovery | `toven modules` over `core`/`contrib`/`examples`/`fuzz` | three Cargo workspaces | Yes | — |
| Publication | per-crate `registry = crates-io`; demos + fuzz excluded | per-crate `publish` | Yes | — |
| Idempotency | `registry-idempotent` readiness | crates.io state | Yes | crates.io upload |
| Hosted release | `host.forge = github` after publish | release workflow | No | signing, SBOM, provenance |

### gokit

gokit publishes Go module tags only (no registry). Every discovered module, including `testutil` and `bench`, is deliberately included and tagged in lock-step, matching `tag-modules.sh`. gokit has no reachable tags yet, so the mutation-free previews fail closed until the first version is cut with the mutating `release tag` action; Toven never fabricates a `0.0.0` baseline.

| Concern | Toven models | Native source of truth | Parity required | Retained native |
|---|---|---|---|---|
| Module discovery | `toven modules` over every `go.mod` | Go workspace | Yes | — |
| Version / tag | path-prefixed `vX.Y.Z` from reachable tags | `tag-modules.sh` | Yes | `tag-modules.sh` push |
| First release | fails closed (no synthetic version) | `make tag VERSION=…` | Yes | initial `make tag` |
| Publication | tag-only, all modules included | `tag-modules.sh` lock-step | Yes | — |
| Artifacts | — | GoReleaser | No | GoReleaser archives, signing, provenance |
