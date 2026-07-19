# Toven

Toven is a fast, argv-first development and CI task planner for multi-module repositories. It discovers workspace modules, orders work by dependency graph, and renders reviewable command batches before execution. The workspace is being rebuilt into a hexagonal `crates/*` (+ future `apps/*`) stack on top of the [rskit](https://github.com/kbukum/rskit) foundation framework (vendored as a git submodule).

## Engineering principles

Apply this baseline to all work here:

- **Phases:** discover → decide (redesign / align / enhance / drop / leave) → implement completely → validate. Toven and rskit are pre-stable: prefer root-cause redesigns over compatibility shims; backward compatibility is not a goal yet.
- **Reuse rskit first:** before writing a shared concern (errors, config, validation, filesystem, git, process, logging), reuse or enhance the canonical rskit owner. If an rskit capability is missing or inadequate, improve rskit generically — never fork a Toven-specific copy or make rskit Toven-specific. Consult [`docs/concern-owners.md`](../docs/concern-owners.md) (rskit-reused vs toven-owned) for the canonical owner of each concern before writing new code.
- **Cascade-complete changes:** a model change flows through schema, normalization, planner, executor, output, tests, and docs in the same change — no half-applied edits.
- **Keep argv unchanged:** user-owned argv is never silently rewritten. Toven validates and expands selectors but does not infer hidden flags. Generated commands are argument vectors by default; shell execution must be opted into explicitly.
- **Libraries do not print:** only the CLI/reporting layer produces user-facing output. Library crates return typed data and typed errors.
- **Typed, minimal APIs:** no broad `Any`-style escape hatches in public surfaces; actionable typed errors that preserve cause.
- **No panics on runtime paths:** no `unwrap()` / `expect()` / swallowed errors outside tests; no success-shaped fallbacks that hide failure.
- **Security:** treat user commands and repository files as untrusted at the CLI boundary; validate at every trust boundary; argv-only subprocess execution; never log secrets; bound input/output.
- **Performance claims require benchmark evidence** (`make benchmark`).
- **Supply chain:** pin CI actions by SHA; enforce dependency/license policy via `cargo-deny`; keep `Cargo.lock` committed; sign release artifacts and attach SBOM/provenance.

The authoritative, longer-form baseline lives in [`docs/engineering.md`](../docs/engineering.md).

Standing, re-runnable development skills encoding this baseline live in [`.github/skills/`](skills/README.md) — the `review` skill runs the review passes in a fresh, clean-context agent after every change set and before releases; `create-branch`, `create-plan`, `apply-plan`, `apply-step`, `commit`, `create-pr`, `fix-reviews`, `validate`, `new-crate`, `rskit-reuse`, `release`, and `docs` cover the rest of the workflow.

## Build, test, and lint

Requires the toolchain pinned in `rust-toolchain.toml` (Rust edition 2024, `rust-version = 1.94`). Initialize submodules first: `git submodule update --init --recursive`.

```bash
make check       # Canonical full gate: fmt-check, lint, test, structure, doc, deny, release build
make fmt         # Format with rustfmt
make fmt-check   # Check formatting without modifying
make lint        # Clippy with -D warnings
make test        # Workspace tests via nextest + doctests (needs cargo-nextest)
make structure   # mod.rs declare-only guard across crates/*
make doc         # Build docs with -D warnings
make deny        # cargo-deny (licenses, advisories, sources)
make coverage    # Coverage gate (cargo-llvm-cov)
```

Prefer validating only the changed modules/crates unless a broader gate is clearly necessary.

## Workspace structure

One Cargo workspace (`members = ["crates/*", "apps/*"]`, `exclude = ["rskit"]`). Layers depend **downward only** (L0 → L1 → L2 → L3 → L4); a lower layer never imports a higher one:

- `crates/toven-model` (L0) — pure vocabulary: identity, dependency graph, plan, and event types plus graph algorithms. The dependency root; it depends on no other Toven crate (only rskit and third-party crates such as `serde`).
- `crates/toven-ports` (L1) — hexagonal port traits (Provider/ConfiguredAdapter, ReleaseTarget, Reporter, RawOutputSink, VcsReader/VcsWriter, ToolchainProber, SourceDigest, CacheStore) and helpers (template, merge, config). Each port is a declare-only responsibility folder. Depends on `toven-model` + rskit.
- `crates/toven-engine` (L2) — PLAN/APPLY coordination and the concrete rskit-backed adapters for the injected ports (e.g. `ProcessToolchainProber`, `FsSourceDigest`, `NullCache`); also owns the strict config `Document` loader.
- adapter crates `crates/toven-{rust,go,command}` (L2) — ecosystem adapters implementing the `toven-ports` traits; never reach into the engine or cli.
- `crates/toven-cli` (L3) — CLI taxonomy, argv-first dispatch, and the stdio/Event projection sinks (the only layer that prints).
- `apps/*` (L4) — thin wiring binaries (`apps/toven`, `apps/toven-rs`, `apps/toven-go`); each wires a set of adapters into `toven-cli`. No new capability lives here — it belongs in the appropriate `crates/*` layer.
- `crates/toven-testkit` — dev-only (`publish = false`) shared test surface: fixtures API, port doubles, sample-repo/git scenario helpers. Tests use it instead of inline TOML.

**Port placement (binding):** a port trait lives in `toven-ports`; its concrete adapter lives in the consuming crate (engine or `toven-<eco>`), never beside the trait; every port has exactly one shared double in `toven-testkit` (`doubles/<port>.rs`) — no port double is stranded inline in a crate's `tests/`. A port trait references only `toven-model` + rskit + std/ports value types; no engine type leaks upward.

The vendored `rskit/` submodule is a separate workspace; Toven depends on individual rskit core crates via path deps pinned to the submodule's prerelease version.

Each crate root (`lib.rs`) and responsibility folder (`mod.rs`) stays **declare-only** — submodule declarations and re-exports only, no logic or private items. Enforced by the `declare-only-aggregator` ast-grep rule (`scripts/sg-rules/`, run via `make structure`), which covers both `lib.rs` and `mod.rs`.

## Code style

- `cargo fmt` (edition 2024, `max_width = 100`) + `cargo clippy` (`all`/`pedantic`/`nursery` warn).
- `unsafe_code = "forbid"` and `missing_docs = "warn"` are set workspace-wide; all public items carry `///` docs.
- `#[must_use]` on `with_*` builder methods; `#[non_exhaustive]` on public enums that may grow.
- No `unwrap()` / `expect()` in library code (tests are fine).
- No test-only escape hatches on production public surfaces: a recover-the-inner accessor used only by tests (`into_inner`, `into_sink`, …) is `#[cfg(test)]`-gated or removed; shared doubles expose recording accessors instead.
- Use rskit's `AppError` / `AppResult` for error handling; preserve the cause.
- Conventional Commits: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.

## Testing

- Behavioral and deterministic; no real network access.
- Use `toven-testkit` fixtures and declarative case files — do not embed large config/TOML strings in tests.
- Cover failure paths; regression-test every fix.
- Runtime paths surface typed errors, never panics.

## Documentation

- Stable project documentation lives in `docs/`; `tmp/` is for active plans/handoff notes only and is never referenced from committed docs.
- Write Markdown paragraphs as natural, continuous source lines. Do not hard-wrap prose to a column limit or insert source newlines for visual presentation; Markdown renderers handle viewport-aware wrapping. Keep intentional structure such as paragraph breaks, headings, lists, blockquotes, tables, mermaid diagrams, and fenced or indented code blocks.
