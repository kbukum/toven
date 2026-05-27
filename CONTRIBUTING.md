# Contributing to Toven

Toven follows Conventional Commits, small reviewable pull requests, and quality
gates that stay close to the implementation phase being changed.

## Local setup

Install the Rust toolchain from `rust-toolchain.toml`, then install the local
tools used by `make check`:

```bash
cargo install cargo-nextest cargo-deny cargo-llvm-cov
```

## Checks

```bash
make check
make coverage
make dist-plan
```

`make check` runs formatting, clippy, tests, docs, cargo-deny, and the
structure guard that keeps product areas in separate internal modules.

Run the checks that match the files you changed before opening a pull request.
Broader gates run in CI as implementation branches add the corresponding
workflow coverage.

## Local CI parity

Use `nektos/act` for workflow parity where GitHub-hosted services are not
required:

```bash
make act-ci
make act-codeql
make act-release-dry-run
```

CodeQL and release signing/provenance remain GitHub-hosted validation paths;
the local substitutes are `make check`, `make coverage`, and `make dist-plan`.

## Commit style

Use Conventional Commits such as `feat: add scheduler model`,
`fix: reject invalid selector macros`, and `docs: explain cache keys`.

## Pull requests

Keep pull requests focused on one reviewable step. Describe the user-visible or
architectural change at a high level, and list validation evidence rather than
restating every changed file.
