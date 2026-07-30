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
| Strict `toven.toml` document loading | `toven-engine` |
| Planning, scheduling, affected selection, cache coordination | `toven-engine` |
| Keyless release signing/verification policy (cosign orchestration) | `toven-engine` |
| Rust ecosystem behavior | `toven-rust` |
| Go ecosystem behavior | `toven-go` |
| CLI parsing and user-facing output | `toven-cli` |
| Shared fixtures and port doubles | `toven-testkit` |

## Port placement

Each port follows one pattern:

1. Trait in `toven-ports`
2. Production adapter in the consuming engine or ecosystem crate
3. One reusable double in `toven-testkit`

Ports reference model, rskit, standard library, or port-owned values. Engine types do not leak upward.

## Decision test

Before adding a shared helper, ask:

1. Is the concern already implemented in rskit?
2. Is it generic enough to belong in rskit?
3. Is it Toven domain behavior?
4. Which layer can own it without creating an upward dependency?
