---
name: apply-step
description: "toven: Implement one plan step test-first, validate its acceptance criteria, and record progress."
user-invocable: true
---

# Applying one plan step, in context

`apply-step` implements exactly one step of a plan folder (from the `create-plan` skill). It is the unit of work that `apply-plan` calls per step, and it can also be run directly on a single step file.

## Input

A path to one step file, e.g. `tmp/engine-plan-caching/02-cache-store.md`.

## 1. Load required context

Read the existing handoff first, then the plan README and current step. Check its dependency status and read only the earlier decisions/contracts it needs. Do not preload every earlier step or re-litigate completed decisions. If a required contract is missing or stale, inspect its owning source before editing.

Confirm the current step's *Depends on* steps are `done` before starting. If a dependency is unfinished, stop and say so. Initialize the submodule if the step touches rskit reuse (`git submodule update --init --recursive`).

## 2. Implement the step against the baseline

Apply the current step's actions **test-first**, honoring Toven's engineering baseline — the plan does not override it, and the authority is [`docs/engineering.md`](../../../docs/engineering.md):

- **TDD.** For each behavior: failing test → minimal code → refactor while green, failure paths included. Use `toven-testkit` fixtures over inline TOML. Never write the production code first.
- **Best-practices bar.** Deliver the *simplest* design that fully solves the step — simple, not shortsighted: keep it flexible and extensible (small typed seams / builders over rigid or speculative abstraction) and scalable (bounded resources, no accidental O(n²) or unbounded buffering). Use current, idiomatic Rust — the right implementation for today's std/spec/security guidance, not folklore. Complexity must earn its place; if a simpler correct design exists, take it and delete the rest.
- **Reuse rskit first.** Before writing a shared concern, open [`docs/concern-owners.md`](../../../docs/concern-owners.md), find the concern's owner (rskit-reused vs toven-owned), and reuse or extend it; improve rskit generically if it is inadequate — never fork a Toven-specific copy.
- **Cascade-complete.** A model change flows through schema, normalization, planner, executor, output, tests, and docs in the same change — no half-applied edits.
- **Placement & layering.** Downward-only (L0 `toven-model` → L1 `toven-ports` → L2 `toven-engine`/`toven-{rust,go,command}` → L3 `toven-cli`); port trait in `toven-ports`, adapter in the consuming crate, one shared double per port in `toven-testkit`; `lib.rs`/`mod.rs` declare-only.
- **Keep argv unchanged; libraries don't print.** User argv is never silently rewritten; only the CLI layer produces user-facing output.
- **Typed & no panic.** No broad `Any` on public surfaces; no `unwrap`/`expect`/swallowed errors on runtime paths; typed `AppError`/`AppResult` preserving cause.
- **Readable files.** Split by concern into focused files; no test-only escape hatches on production public surfaces (`#[cfg(test)]`-gate or remove them).

Keep the edit scoped to *this* step's `Files touched`; if you discover the step is mis-scoped, report it rather than silently expanding.

## 3. Validate, review, and mark done

- **Validate** the affected crates with the `validate` skill (`cargo -p` + `make structure`), deterministic and green. A step does not land red.
- **Review** the step's diff with the relevant `review` passes (structure/placement, rskit reuse, principles, quality, tests, docs, comments) in the current agent; delegate only when the user requests it.
- Only when acceptance criteria are genuinely met, flip the step's progress signal so `apply-plan` can resume: set `**Status:** done` and check its `- [x]` boxes. Do not mark a step done on a partial or red result.

Update `handoff.md` with the completed capability, remaining work, exact next action, validation freshness, Git constraints, and owned resources. Keep it under 500 words.

## Repo workflow

Work on a branch (`create-branch` skill), leave edits **uncommitted** for the maintainer to commit and push; open a PR only when explicitly asked. When that change becomes a branch/PR, name it by the change — never `step-2` or a plan/batch number.
