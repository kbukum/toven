# Toven

Argv-first development and CI task planner, alpha and pre-stable. `crates/*` own capabilities; `apps/*` only wire binaries. The vendored `rskit/` submodule is a separate workspace; do not change its revision as a side effect.

[`docs/engineering.md`](../docs/engineering.md) owns the rules. This file lists the ones every task needs and where to read the rest.

## Invariants

- Fix root causes; no compatibility shims. Pick Redesign / Align / Enhance / Drop for each defect.
- Reuse the [rskit owner](../docs/concern-owners.md) for shared concerns; enhance rskit generically instead of forking.
- Dependencies point downward. Port traits live in `toven-ports`, adapters live with their consumer, and each port has one double in `toven-testkit`.
- Model changes cascade through schema, normalization, planner, executor, output, tests, and docs together.
- Never rewrite user argv. Shell execution is explicit opt-in. Treat config, repo files, and argv as untrusted; bound I/O; honor cancellation.
- Libraries return typed data and `AppError` / `AppResult`; only `toven-cli` prints. No runtime `unwrap`/`expect`, swallowed errors, or success-shaped fallbacks.
- `lib.rs` / `mod.rs` are declare-only. Test first, with `toven-testkit` fixtures and failure paths.

## Work and validation

- Preserve the worktree and index. Commit, amend, push, create branches, or open draft PRs only when authorized.
- Keep plans in `tmp/<plan>/`, reusing existing folders. Apply a selected step fully and record progress in it.
- Read only the matching [skill](skills/README.md) and the engineering sections it needs.
- Validate with the [validate skill](skills/validate/SKILL.md): scoped `cargo -p` checks first, `make check` for the full gate. Prose-only edits need doc checks, not a full build.
- Stable docs live in `docs/` and never link `tmp/`. Do not hard-wrap Markdown or Rust prose.

## Where to read

| Task | Section |
|---|---|
| Crate, module, port, or adapter placement | [Placement and layering](../docs/engineering.md#placement-and-layering), [architecture](../docs/architecture.md) |
| Shared helpers | [Reuse rskit first](../docs/engineering.md#reuse-rskit-first) |
| Argv, subprocesses, secrets, concurrency | [Security and runtime correctness](../docs/engineering.md#security-and-runtime-correctness) |
| Public types, errors, flags, output | [Public API and stability](../docs/engineering.md#public-api-and-stability), [CLI and consumer experience](../docs/engineering.md#cli-and-consumer-experience) |
| Tests | [Tests and proof](../docs/engineering.md#tests-and-proof), [testing](../docs/testing.md) |
| Docs and comments | [Documentation](../docs/engineering.md#documentation) |
| Dependencies, CI, release | [Supply chain and release](../docs/engineering.md#supply-chain-and-release) |
| What a gate checks vs. review only | [Gates](../docs/engineering.md#gates), [known gaps](../docs/engineering.md#known-gaps) |
| Finishing a change | [Definition of Done](../docs/engineering.md#definition-of-done) |
