# Contributing to Toven

Thanks for your interest in Toven. Toven follows Conventional Commits, small reviewable pull requests, and quality gates that stay close to the implementation area being changed.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security issues follow [SECURITY.md](SECURITY.md), not public issues.

## Project status

Toven is in **alpha** — signed binaries are published on the [Releases page](https://github.com/kbukum/toven/releases) — and still consolidating into a hexagonal `crates/*` + `apps/*` stack on top of the [rskit](https://github.com/kbukum/rskit) foundation framework. Toven and rskit are both pre-stable: backward compatibility is not a goal yet, so prefer clean redesigns over compatibility shims. See [GOVERNANCE.md](GOVERNANCE.md) for how decisions are made.

## Local setup

Toven vendors rskit as a git submodule, so initialize submodules before building:

```bash
git submodule update --init --recursive
```

Install the toolchain pinned in `rust-toolchain.toml`, then the local tools `make check`/`make coverage` need: `cargo-nextest` for `make test`, `ast-grep` for `make structure`, `cargo-deny` for the supply-chain gate, and `cargo-llvm-cov` for coverage:

```bash
cargo install cargo-nextest --locked
cargo install ast-grep --locked
cargo install cargo-deny --locked --version 0.19.0
cargo install cargo-llvm-cov --locked --version 0.8.5
```

## Checks

```bash
make check
make coverage
```

`make check` runs formatting, clippy, workspace tests, docs, the dependency/license audit, the `mod.rs`/structure guard, and a release build. `make coverage` enforces the current coverage threshold. `toven release package --target <triple>` archives a built binary into its declared hosted-release asset without publishing.

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

One Cargo workspace (`members = ["crates/*", "apps/*"]`, `exclude = ["rskit"]`), with layers depending downward only:

- `crates/toven-model` (L0) — pure vocabulary (identity, dependency graph, plan, event types) plus graph algorithms; the dependency root.
- `crates/toven-ports` (L1) — hexagonal port traits and helpers (template, merge, config).
- `crates/toven-engine-core`, `crates/toven-engine-release`, `crates/toven-engine` (L2) — the PLAN foundation, the release PLAN/APPLY tail, and the rest of PLAN/APPLY coordination.
- `crates/toven-rust`, `crates/toven-go`, `crates/toven-command` (L2) — ecosystem adapters over the ports.
- `crates/toven-cli` (L3) — CLI taxonomy, argv-first dispatch, and the stdio/Event projection sinks.
- `apps/toven`, `apps/toven-rs`, `apps/toven-go` (L4) — thin wiring binaries.
- `crates/toven-testkit` — dev-only (`publish = false`) shared test surface: fixtures API, port doubles, and sample-repo/git scenario helpers.

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

Keep pull requests focused on one reviewable step. Use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md): describe the user-visible or architectural change with a high-level, itemized summary, and list the validation evidence rather than restating every changed file (GitHub already shows that in the Files changed tab).

For significant design changes, open a discussion or issue before implementation so maintainers can align on direction early.
