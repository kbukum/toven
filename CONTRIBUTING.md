# Contributing to Toven

Toven follows Conventional Commits, small reviewable pull requests, and quality
gates that stay close to the implementation phase being changed.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Local setup

Install the Rust toolchain from `rust-toolchain.toml`. The required formatting,
linting, test, documentation, and build checks run through standard Cargo
commands.

## Checks

```bash
make check
make coverage
make dist-plan
```

`make check` runs formatting, clippy, tests, docs, metadata validation, and a
release build. `make coverage` requires `cargo-llvm-cov` and is run separately
until coverage CI lands.

Run the checks that match the files you changed before opening a pull request.
Broader gates run in CI as implementation branches add the corresponding
workflow coverage.

## Local CI parity

Use `nektos/act` for pull request workflow parity where GitHub-hosted services
are not required:

```bash
make act-ci
```

CodeQL and release signing/provenance remain GitHub-hosted validation paths; the
local substitutes are `make check`, `make coverage`, and `make dist-plan`.

## Commit style

Use Conventional Commits such as `feat: add scheduler model`,
`fix: reject invalid selector macros`, and `docs: explain cache keys`.

## Pull requests

Keep pull requests focused on one reviewable step. Describe the user-visible or
architectural change at a high level, and list validation evidence rather than
restating every changed file.

For significant design changes, open a discussion or issue before implementation
so maintainers can align on the direction early.
