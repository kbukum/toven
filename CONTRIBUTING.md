# Contributing to Toven

Set up a local checkout, run the right gates for your change, and open a small reviewable pull request.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security issues follow [SECURITY.md](SECURITY.md), not public issues.

## Quick start

Toven vendors rskit as a git submodule, so initialize submodules before building:

```bash
git submodule update --init --recursive
```

Install the toolchain pinned in `rust-toolchain.toml`, then install the local tools the main gates use:

```bash
cargo install cargo-nextest --locked
cargo install ast-grep --locked
cargo install cargo-deny --locked --version 0.19.0
cargo install cargo-llvm-cov --locked --version 0.8.5
```

Run the main local gates:

```bash
make check
make coverage
```

`make doctor` checks that the tools are installed. `make check` is the canonical gate and `make coverage` enforces the coverage floor; the [gates section](docs/engineering.md#gates) says what each one checks. `make verify-release-platform` is the focused release go/no-go: it fails on an empty scenario filter or a scenario skipped for a missing tool.

If you are working on packaging or release behavior, `toven release package --target <triple>` archives a built binary into its declared hosted-release asset without publishing it.

## Project status

Toven is in **alpha**. Signed binaries are published on the [Releases page](https://github.com/kbukum/toven/releases). The workspace is a hexagonal `crates/*` + `apps/*` stack on top of the [rskit](https://github.com/kbukum/rskit) foundation framework. Toven and rskit are both pre-stable, so prefer clean redesigns over compatibility shims; see the [stability policy](docs/engineering.md#public-api-and-stability). See [GOVERNANCE.md](GOVERNANCE.md) for how decisions are made.

## Checks

Run the checks that match the files you changed before opening a pull request; prefer targeted checks for the changed crate. Broader gates run in CI.

### Local CI parity

Use [`nektos/act`](https://github.com/nektos/act) for pull request workflow parity where GitHub-hosted services are not required:

```bash
make act-ci
make act-supply-chain
make act-release-readiness
```

CodeQL, artifact signing, and provenance attestations remain GitHub-hosted validation paths; the local substitutes are `make check`, `make coverage`, and the engine release verbs (`toven release plan | package | checksums | verify`).

## Where code goes and the rules it follows

- [Architecture](docs/architecture.md) owns the crate layers and where new code belongs.
- The [engineering standard](docs/engineering.md) owns code, test, docs, and security rules, and marks which ones a gate checks.
- [Testing](docs/testing.md) explains the fixtures and the test layout.
- The [Definition of Done](docs/engineering.md#definition-of-done) is the checklist a pull request must meet.

## Commit style

Use Conventional Commits, such as `feat: add scheduler model`, `fix: reject invalid selector macros`, and `docs: explain cache keys`. CI checks the pull request title.

## Pull requests

Keep pull requests focused on one reviewable step. Use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md): describe the user-visible or architectural change with a short itemized summary, then list the validation evidence instead of restating every changed file.

For significant design changes, open a discussion or issue before implementation so maintainers can align on direction early.
