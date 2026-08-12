# Concern ownership

Shared concerns have one canonical implementation owner. Reuse the owner before adding behavior elsewhere.

## rskit-owned concerns

| Concern | Owner |
|---|---|
| Application errors and results | rskit errors |
| Validation primitives | rskit validation |
| Filesystem operations | rskit filesystem |
| Deterministic archive packaging (tar.gz/zip) | rskit filesystem |
| Git operations | rskit Git |
| Process execution and observation | rskit process |
| General configuration primitives | rskit configuration |
| Logging infrastructure | rskit logging |
| SHA-256 digests (checksums/manifests) | rskit util |

When a shared capability is missing, improve rskit generically. Do not make rskit depend on Toven concepts.

## Toven-owned concerns

| Concern | Owner |
|---|---|
| Module identity and dependency graph | `toven-model` |
| Task and release vocabulary | `toven-model` and `toven-ports` |
| Port traits | `toven-ports` |
| Strict `toven.toml` document loading | `toven-core` |
| PLAN spine and federation-core (resolve/baseline/compose) | `toven-core` |
| Engine-owned VCS baseline policy over the git seam | `toven-core` |
| Path→owning-module resolver | `toven-core` |
| Semver bump math and release-tag codec/selection (`next_version`, `TagScheme`, `latest_matching`) | `toven-semver` |
| Git mechanism: the rskit-git-backed `VcsReader`/`VcsWriter` adapter, the change foundation (diff-range resolution), and the per-repo reader-set fan-out | `toven-vcs` |
| Version decision: the pure `plan_bumps` bump/cascade/idempotency decision, baseline anchoring, entrypoint/`CutIntent` policy, change detection, and Conventional-Commit changelog generation | `toven-version` |
| Concrete subprocess runners (`ProcessToolRunner`, `ProcessCommandRunner`, persistent spawn) and the shared argv→`ProcessSpec` lowering | `toven-exec` |
| Scheduling, affected selection, apply, cache coordination, coverage | `toven-engine` |
| Release flow orchestration (the ordered tag/package/SBOM/sign/publish/host phases composing `toven-version` for the bump decision) | `toven-release` |
| Keyless release signing/verification policy (cosign orchestration) | `toven-release` |
| Rust ecosystem behavior | `toven-rust` |
| Go ecosystem behavior | `toven-go` |
| CLI parsing and user-facing output | `toven-cli` |
| Shared fixtures and port doubles | `toven-testkit` |

## Port placement

Each port follows one pattern:

1. Trait in `toven-ports`
2. Production adapter in the consuming crate — an engine or ecosystem crate, or a focused mechanism crate such as `toven-exec`/`toven-vcs`
3. One reusable double in `toven-testkit`

Ports reference model, rskit, standard library, or port-owned values. Engine types do not leak upward.

## Decision test

Before adding a shared helper, ask:

1. Is the concern already implemented in rskit?
2. Is it generic enough to belong in rskit?
3. Is it Toven domain behavior?
4. Which layer can own it without creating an upward dependency?
