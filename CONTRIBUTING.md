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

`make check` is the canonical gate. It runs formatting, clippy, workspace tests, rustdoc, the declare-only structure guard, the dependency and license audit, release-platform verification, and a release build. `make coverage` enforces the configured coverage floor.

If you are working on packaging or release behavior, `toven release package --target <triple>` archives a built binary into its declared hosted-release asset without publishing it.

## Project status

Toven is in **alpha**. Signed binaries are published on the [Releases page](https://github.com/kbukum/toven/releases), and the workspace is still settling into a hexagonal `crates/*` + `apps/*` stack on top of the [rskit](https://github.com/kbukum/rskit) foundation framework. Toven and rskit are both pre-stable, so prefer clean redesigns over compatibility shims. See [GOVERNANCE.md](GOVERNANCE.md) for how decisions are made.

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

## Workspace layout

One Cargo workspace (`members = ["crates/*", "apps/*"]`, `exclude = ["rskit"]`) with layers depending downward only. At a high level:

- **Foundations:** `crates/toven-model`, `crates/toven-semver`
- **Ports and focused mechanisms:** `crates/toven-ports`, `crates/toven-exec`, `crates/toven-vcs`
- **Planning and release engines:** `crates/toven-core`, `crates/toven-version`, `crates/toven-release`, `crates/toven-engine`, `crates/toven-runtime`
- **Ecosystem adapters:** `crates/toven-rust`, `crates/toven-go`, `crates/toven-command`
- **CLI and app binaries:** `crates/toven-cli`, `apps/toven`, `apps/toven-rs`, `apps/toven-go`
- **Shared test surface:** `crates/toven-testkit` (`publish = false`) for fixtures, port doubles, and sample-repo/git scenarios

The vendored `rskit/` submodule is a separate workspace consumed via path deps.

## Code conventions

- `cargo fmt` (edition 2024, `max_width = 100`) and `cargo clippy` (`all`/`pedantic`/`nursery` warn) must be clean.
- `unsafe_code = "forbid"` and `missing_docs = "warn"` apply workspace-wide; document public items with `///`.
- `#[must_use]` on `with_*` builder methods; `#[non_exhaustive]` on public enums that may grow.
- No `unwrap()` / `expect()` in library code (tests are fine); surface typed `AppError` / `AppResult` and preserve the cause.
- Libraries never print — only the CLI/reporting layer produces user-facing output.
- Reuse or enhance the canonical rskit owner for shared concerns (errors, config, validation, filesystem, git) instead of forking a Toven-specific copy. If rskit is missing something, improve it generically.

## Testing

- Tests are behavioral and deterministic, with no real network access.
- Use `toven-testkit` fixtures and declarative case files instead of embedding large config/TOML strings in tests.
- Cover failure paths and add a regression test for every fix.

## Commit style

Use Conventional Commits, such as `feat: add scheduler model`, `fix: reject invalid selector macros`, and `docs: explain cache keys`.

## Pull requests

Keep pull requests focused on one reviewable step. Use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md): describe the user-visible or architectural change with a short itemized summary, then list the validation evidence instead of restating every changed file.

For significant design changes, open a discussion or issue before implementation so maintainers can align on direction early.
