# Contributing to Toven

Toven follows Conventional Commits, small reviewable pull requests, and quality
gates that stay close to the implementation phase being changed.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Local setup

Install the Rust toolchain from `rust-toolchain.toml`, then install the local
tools used by the supply-chain and coverage gates:

```bash
cargo install cargo-deny --locked --version 0.19.0
cargo install cargo-llvm-cov --locked --version 0.8.5
```

## Checks

```bash
make check
make coverage
make smoke
make dist-plan
make release-artifacts
```

`make check` runs formatting, clippy, tests, docs, dependency/license audit,
metadata validation, release packaging dry-run, and a release build.
Managed smoke cases run through the normal test target; `make smoke` is the
focused binary-level real-repository smoke gate. `make coverage` enforces the
current coverage threshold. `make release-artifacts` stages the crate archive
and checksum manifest without publishing.

Run the checks that match the files you changed before opening a pull request.
Broader gates run in CI as implementation branches add the corresponding
workflow coverage.

## Local CI parity

Use `nektos/act` for pull request workflow parity where GitHub-hosted services
are not required:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

CodeQL, artifact signing, and provenance attestations remain GitHub-hosted
validation paths; the local substitutes are `make check`, `make coverage`, and
`make release-artifacts`.

## Commit style

Use Conventional Commits such as `feat: add scheduler model`,
`fix: reject invalid selector macros`, and `docs: explain cache keys`.

## Pull requests

Keep pull requests focused on one reviewable step. Describe the user-visible or
architectural change at a high level, and list validation evidence rather than
restating every changed file.

For significant design changes, open a discussion or issue before implementation
so maintainers can align on the direction early.
