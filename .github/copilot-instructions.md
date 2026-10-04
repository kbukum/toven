# Toven

Argv-first development/CI task planner. `crates/*` owns capabilities; `apps/*` only wires binaries. The vendored `rskit/` submodule is a separate workspace; do not change its revision incidentally. Use the pinned toolchain and workspace lints.

## Invariants

- Pre-stable: fix root causes, not compatibility shims. Choose Redesign / Align / Enhance / Drop for defects; leave sound code alone.
- Reuse [canonical rskit owners](../docs/concern-owners.md) for shared infrastructure. Enhance rskit generically if needed; never fork a Toven-specific helper. Read `rskit-reuse` before adding shared concerns.
- Dependencies point from higher consumers to lower contracts. `toven-model` is vocabulary; `toven-ports` owns port traits; concrete adapters live with consumers, never beside traits. Each port has one shared double in `toven-testkit`. Read [workspace structure](engineering.md#workspace-structure) before changing crate/port boundaries.
- Model changes propagate through schema, normalization, planner, executor, output, tests, and docs together. User argv is never silently rewritten; shell execution is explicit opt-in.
- Libraries return typed data and `AppError` / `AppResult`; only CLI/reporting prints. No runtime panics, unwrap/expect, swallowed errors, or success-shaped fallbacks.
- Treat commands and repo files as untrusted input. Validate boundaries, redact secrets, bound I/O, and own process/task cancellation and shutdown. Inject clients and policies, not globals.
- `lib.rs` / `mod.rs` are declare-only. Use concern-named modules, inherited `unsafe_code = "forbid"` and documentation lints, documented public APIs, `#[must_use]` builders, and `#[non_exhaustive]` growing enums. No production API added solely for tests.
- Test-first; deterministic fixtures and port doubles from `toven-testkit`, not embedded TOML or one-off fakes. Cover failures. Performance claims require `make benchmark` evidence.
- Keep `Cargo.lock`, SHA-pinned actions, dependency/license checks, signed release artifacts, SBOM, and provenance. No secrets in retained artifacts.

## Work and validation

Read only the matching [skill](skills/README.md) and relevant reference sections. Preserve worktree/index changes; commit, amend, push, or open draft PRs only when authorized. Multi-step recovery lives in `tmp/plans/<task>/handoff.md`; read current work and required dependency contracts, not every past step.

Use [validate](skills/validate/SKILL.md) for scoped Cargo checks. `make check` is the canonical full gate; `make fmt-check`, `make structure`, `make doc`, `make deny`, and `make coverage` cover specific concerns. Initialize submodules only if required and absent. Prose-only edits need documentation checks, not a full build.

## References

Read [code style](engineering.md#code-style) for APIs/module organization, [testing](engineering.md#testing) for fixtures, and [engineering principles](engineering.md#engineering-principles) for safety/release changes. Stable product docs live in `docs/`; do not link them to temporary task notes. Markdown and Rust prose are not hard-wrapped. Do not preload [the full reference](engineering.md).
