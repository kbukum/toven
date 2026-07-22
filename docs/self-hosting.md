# Self-hosting and CI

Toven uses its own planner for mapped development and release gates. The Makefile remains the stable local and CI entry point.

## Binary selection

By default, Make targets run the freshly built workspace binary:

```makefile
TOVEN ?= cargo run --quiet --locked -p toven --
```

Use an installed binary:

```bash
make TOVEN=toven check
```

## Canonical gate

```bash
make check
```

The gate includes formatting, linting, nextest, doctests, structure checks, rustdoc, dependency policy, and release build readiness.

Mapped task gates run through Toven. Native gates remain native when Toven does not own their concern:

| Gate | Execution |
|---|---|
| Lint, nextest, rustdoc, release build | Toven task |
| rustfmt workspace check | Native Cargo |
| Rust doctests | Native Cargo |
| Dependency policy | cargo-deny |
| Declare-only structure | ast-grep |

## Additional gates

```bash
make affected
make coverage
make release-plan
make smoke
```

- `affected` previews changed test scope.
- `coverage` measures and applies configured thresholds.
- `release-plan` runs release plan, readiness, SBOM, and dependency graph previews.
- `smoke` drives built application binaries over committed fixtures.

## CI output

Human task progress and summaries use stderr. Read-only tables and JSONL use stdout. CI should capture both streams while parsing only stdout when machine-readable output is requested.

```bash
toven release plan --output jsonl > release-plan.jsonl
```

## Release CI model

Release CI should:

1. Install a pinned Toven binary.
2. Verify its checksum.
3. Run plan, readiness, SBOM, dependency graph, and dry-run commands.
4. Preserve machine-readable evidence.
5. Require a protected environment approval.
6. Run real publication with least-privilege permissions.
7. Verify tags, registry state, hosted assets, signatures, SBOM, and provenance.

Until released binaries are available, Toven builds itself from the checkout. Downstream repositories must not depend on an unreleased binary.

## GitHub Action direction

The planned `toven-action` is a thin installer and command forwarder. It downloads a selected Toven release, verifies integrity, optionally caches the binary, and forwards argv unchanged. Release policy remains in `toven.toml` and Toven itself.

Pin the action to an immutable commit SHA and the binary to a version and checksum.

## Local workflow reproduction

When `act` is installed:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

These commands reproduce workflow structure locally but do not replace GitHub-hosted release verification.
