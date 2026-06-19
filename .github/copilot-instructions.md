# Toven

Toven is a fast, argv-first development and CI task planner for multi-module repositories. It discovers workspace modules, orders work by dependency graph, and renders reviewable command batches before execution. The workspace is being rebuilt into a hexagonal `crates/*` (+ future `apps/*`) stack on top of the [rskit](https://github.com/kbukum/rskit) foundation framework (vendored as a git submodule).

## Engineering principles

Apply this baseline to all work here:

- **Phases:** discover → decide (redesign / align / enhance / drop / leave) → implement completely → validate. Toven and rskit are pre-stable: prefer root-cause redesigns over compatibility shims; backward compatibility is not a goal yet.
- **Reuse rskit first:** before writing a shared concern (errors, config, validation, filesystem, git, process, logging), reuse or enhance the canonical rskit owner. If an rskit capability is missing or inadequate, improve rskit generically — never fork a Toven-specific copy or make rskit Toven-specific.
- **Cascade-complete changes:** a model change flows through schema, normalization, planner, executor, output, tests, and docs in the same change — no half-applied edits.
- **argv is sacred:** user-owned argv is never silently rewritten. Toven validates and expands selectors but does not infer hidden flags. Generated commands are argument vectors by default; shell execution must be opted into explicitly.
- **Libraries do not print:** only the CLI/reporting layer produces user-facing output. Library crates return typed data and typed errors.
- **Typed, minimal APIs:** no broad `Any`-style escape hatches in public surfaces; actionable typed errors that preserve cause.
- **No panics on runtime paths:** no `unwrap()` / `expect()` / swallowed errors outside tests; no success-shaped fallbacks that hide failure.
- **Security:** treat user commands and repository files as untrusted at the CLI boundary; validate at every trust boundary; argv-only subprocess execution; never log secrets; bound input/output.
- **Performance claims require benchmark evidence** (`make benchmark`).
- **Supply chain:** pin CI actions by SHA; enforce dependency/license policy via `cargo-deny`; keep `Cargo.lock` committed; sign release artifacts and attach SBOM/provenance.

The authoritative, longer-form baseline lives in [`docs/engineering.md`](../docs/engineering.md).

## Build, test, and lint

Requires the toolchain pinned in `rust-toolchain.toml` (Rust edition 2024, `rust-version = 1.94`). Initialize submodules first: `git submodule update --init --recursive`.

```bash
make check       # Canonical full gate: fmt-check, lint, test, structure, doc, deny, release build
make fmt         # Format with rustfmt
make fmt-check   # Check formatting without modifying
make lint        # Clippy with -D warnings
make test        # Workspace cargo tests
make structure   # mod.rs declare-only guard across crates/*
make doc         # Build docs with -D warnings
make deny        # cargo-deny (licenses, advisories, sources)
make coverage    # Coverage gate (cargo-llvm-cov)
```

Prefer validating only the changed modules/crates unless a broader gate is clearly necessary.

## Workspace structure

One Cargo workspace (`members = ["crates/*"]`, `exclude = ["rskit"]`). Layers depend downward only:

- `crates/toven-model` — pure vocabulary: identity, dependency graph, plan, and event types plus graph algorithms. The dependency root; it depends on no other Toven crate (only rskit and third-party crates such as `serde`).
- `crates/toven-ports` — hexagonal port traits (Provider/ConfiguredAdapter, ReleaseTarget, Reporter, VcsReader/VcsWriter) and helpers (template, merge, config). Depends on `toven-model` + rskit.
- `crates/toven-testkit` — dev-only (`publish = false`) shared test surface: fixtures API, port doubles, sample-repo/git scenario helpers. Tests use it instead of inline TOML.

The vendored `rskit/` submodule is a separate workspace; Toven depends on individual rskit core crates via path deps pinned to the submodule's prerelease version.

## Code style

- `cargo fmt` (edition 2024, `max_width = 100`) + `cargo clippy` (`all`/`pedantic`/`nursery` warn).
- `unsafe_code = "forbid"` and `missing_docs = "warn"` are set workspace-wide; all public items carry `///` docs.
- `#[must_use]` on `with_*` builder methods; `#[non_exhaustive]` on public enums that may grow.
- No `unwrap()` / `expect()` in library code (tests are fine).
- Use rskit's `AppError` / `AppResult` for error handling; preserve the cause.
- Conventional Commits: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.

## Testing

- Behavioral and deterministic; no real network access.
- Use `toven-testkit` fixtures and declarative case files — do not embed large config/TOML strings in tests.
- Cover failure paths; regression-test every fix.
- Runtime paths surface typed errors, never panics.

## Documentation

- Stable project documentation lives in `docs/`; `tmp/` is for active plans/handoff notes only and is never referenced from committed docs.
- Markdown is not hard-wrapped: write one line per paragraph (no mid-sentence line breaks); preserve code blocks, tables, and lists.
