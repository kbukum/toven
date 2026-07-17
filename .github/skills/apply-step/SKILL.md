---
name: apply-step
description: >-
    Apply a single step of a tmp/ plan — read the plan README and all previous steps for
    accumulated context and decisions, then implement the current step test-first against
    Toven's engineering baseline, validate the affected crates, and mark the step done. Use to
    execute one specific plan step, or as the per-step unit that apply-plan drives.
user-invocable: true
---

# Applying one plan step, in context

`apply-step` implements exactly one step of a plan folder (from the `create-plan` skill). It is the unit of work that `apply-plan` calls per step, and it can also be run directly on a single step file.

## Input

A path to one step file, e.g. `tmp/engine-plan-caching/02-cache-store.md`.

## 1. Load full context before editing

A step is not self-contained — earlier steps make naming, layering, and API decisions this step depends on. Read, in order:

1. **`README.md`** of the plan folder — goal, dependency order, and the cross-cutting baseline rules that bind every step.
2. **Every previous step** (`NN-*.md` with a lower number) — for the decisions and files they already established. Honor them; do not re-litigate or contradict a completed step.
3. **The current step** — its scope, numbered actions, files touched, and acceptance criteria.

Confirm the current step's *Depends on* steps are `done` before starting. If a dependency is unfinished, stop and say so. Initialize the submodule if the step touches rskit reuse (`git submodule update --init --recursive`).

## 2. Implement the step against the baseline

Apply the current step's actions **test-first**, honoring Toven's engineering baseline — the plan does not override it, and the authority is [`docs/engineering.md`](../../../docs/engineering.md):

- **TDD.** For each behavior: failing test → minimal code → refactor while green, failure paths included. Use `toven-testkit` fixtures over inline TOML. Never write the production code first.
- **Reuse rskit first.** Before writing a shared concern, open [`docs/concern-owners.md`](../../../docs/concern-owners.md), find the concern's owner (rskit-reused vs toven-owned), and reuse or extend it; improve rskit generically if it is inadequate — never fork a Toven-specific copy.
- **Cascade-complete.** A model change flows through schema, normalization, planner, executor, output, tests, and docs in the same change — no half-applied edits.
- **Placement & layering.** Downward-only (L0 `toven-model` → L1 `toven-ports` → L2 `toven-engine`/`toven-{rust,go,command}` → L3 `toven-cli`); port trait in `toven-ports`, adapter in the consuming crate, one shared double per port in `toven-testkit`; `lib.rs`/`mod.rs` declare-only.
- **Keep argv unchanged; libraries don't print.** User argv is never silently rewritten; only the CLI layer produces user-facing output.
- **Typed & no panic.** No broad `Any` on public surfaces; no `unwrap`/`expect`/swallowed errors on runtime paths; typed `AppError`/`AppResult` preserving cause.
- **Readable files.** Split by concern into focused files; no test-only escape hatches on production public surfaces (`#[cfg(test)]`-gate or remove them).

Keep the edit scoped to *this* step's `Files touched`; if you discover the step is mis-scoped, report it rather than silently expanding.

## 3. Validate, review, and mark done

- **Validate** the affected crates with the `validate` skill (`cargo -p` + `make structure`), deterministic and green. A step does not land red.
- **Review** the step's diff with the relevant `review` passes (structure/placement, rskit reuse, principles, quality, tests, docs, comments) — ideally in a fresh agent.
- Only when acceptance criteria are genuinely met, flip the step's progress signal so `apply-plan` can resume: set `**Status:** done` and check its `- [x]` boxes. Do not mark a step done on a partial or red result.

## Repo workflow

Work on a branch (`create-branch` skill), leave edits **uncommitted** for the maintainer to commit and push; open a PR only when explicitly asked. When that change becomes a branch/PR, name it by the change — never `step-2` or a plan/batch number.
